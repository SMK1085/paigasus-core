<!-- SPDX-License-Identifier: Apache-2.0 -->

# `repo:ruff-ci`

Lints `ci/**/*.py` against `py/pyproject.toml`'s Ruff rule set (SMA-539).

## Why

`.moon/tasks/python.yml` scopes `ruff check` to the `py` project. `ci/` has never been
linted by anything, and it merged through a full review carrying three RUF005
violations before Task 1 of this issue cleared them (SMA-541). This gate closes that
gap without introducing a second Ruff configuration: the rule set, its version, and its
resolution all come from `py/pyproject.toml` + `py/uv.lock`. There is no
`ci/ruff/pyproject.toml` and no second lockfile — a violation here is the same violation
it would be in `py/`.

**`per-file-ignores` is not banned — it ships unused.** An earlier draft of this gate
banned `[tool.ruff.lint.per-file-ignores]` outright, which contradicted this repo's own
idiom: `T_EXEMPT`, `ALLOW_DEAD_INPUT`, `BRANCH_SKIP`, `COE_SKIP`, `ALLOW_UNLOCKED_CARGO`
and every other exemption table in this repo ships a *reasoned* exception, not a
blanket refusal. The mechanism is available the same way, requiring a stated reason for
any entry; it simply has none today. Every violation this gate found when it was first
run over `ci/` (the three `RUF005`s above, and every finding from the full-corpus review
that followed) was fixed rather than exempted, so there is nothing latent this hatch is
hiding — a future genuine exception (a fixture file that must contain deliberately bad
style, say) has a documented way in instead of forcing a hack or a spec amendment.

## What it asserts

`run.sh` derives the set of tracked `ci/**/*.py` files, resolves the `ruff` binary from
`py`'s locked environment, and runs `ruff check --config py/pyproject.toml` against that
exact file list. Nothing else. It does not lint `py/` (already gated by
`.moon/tasks/python.yml`), and it does not run `ruff format` (see below).

## `REPO_ROOT` is computed unconditionally, with no override

`run.sh` derives `REPO_ROOT` from `BASH_SOURCE` alone, matching every other
`ci/*/run.sh` in this repo. It does not honour a pre-set `REPO_ROOT` environment
variable. An earlier draft of this script did honour one, meant only to let
`self_test`/`negative_control` point the whole script at a throwaway tree — but the
override applied unconditionally, so `REPO_ROOT=<some other worktree> bash
ci/ruff/run.sh`, with no flag at all, silently linted that other tree and reported a
clean "10 files clean" at rc 0 for the real one. A gate that lints the wrong tree and
exits 0 is exactly the failure this issue exists to prevent, so the override was
removed rather than narrowed. `self_test`'s empty-corpus check still needs this trick —
it copies this script into a throwaway tree and invokes that copy directly, so the
copy's own `BASH_SOURCE` resolves `REPO_ROOT` to the fixture naturally, with no
environment surface at all. `negative_control` no longer uses it: since PR 206 (see
"Why the negative control never calls `uv`" below) it never re-executes any copy of
this script — it resolves `ruff` once in the real repo and invokes that binary
directly against its fixture's files, so there is no second `REPO_ROOT` to resolve.

## Exit codes, and why 1 and 2 must not collapse into each other

`0` pass, `1` the repo is wrong, `2` infrastructure failed — the repo's usual contract.

The reason this gate is two steps (resolve the binary, then invoke it directly) instead
of one (`uv run --project py -- ruff check ...`) is that **both halves of that combined
command exit 1 on their own failure**: `ruff check` exits 1 on a lint violation, and `uv`
exits 1 on a failed dependency resolution and on `--locked` finding a stale lock. A
single piped invocation cannot tell "`ci/` has lint violations" apart from "PyPI is
down" — both would read as the same rc 1. CLAUDE.md records this exact lesson for
`repo:workflow-credentials`, which hit it first.

`negative_control` resolves `ruff` the same way, but only ONCE, up front, in the real
repo — see "Why the negative control never calls `uv`" below for why calling it a
second time, from inside the fixture, is itself a bug this gate used to have.

`resolve_ruff` isolates the `uv`-shaped failure: it resolves the `ruff` executable's
path via `uv run --locked --project py python3 -c '...shutil.which("ruff")...'`, and any
non-zero exit or a non-executable result there is reported at **rc 2** with an
infrastructure message. Only once that has succeeded does `run.sh` invoke `"$ruff"
check` directly — no `uv` in that second command at all — so a violation it reports can
only mean rc 1. This is also why `.moon/tasks/python.yml`'s bare, re-locking `uv run
ruff check .` is not reused here: `py/uv.lock` can genuinely be stale in a working tree,
and without the split that reds this gate for a reason a contributor would "fix" by
re-locking a file this gate has no business touching.

Verified live: temporarily pointing `resolve_ruff` at a `uv` project with no `ruff`
installed (`--project ci/release-plan`) produces

```
ruff-ci: could not resolve ruff via 'uv run --locked --project py' — run 'uv sync --project py'
```

at **rc 2**, never rc 1.

**Provenance, not just presence (whole-branch review, I1).** `shutil.which` is
PATH-based, and that opened a gap "resolve, then assert `[ -x ]`" alone didn't close:
if the `py` uv project does not actually contain `ruff` — a `[dependency-groups]`
rename, a `[tool.uv] default-groups` change, or `UV_NO_DEV=1` at invocation time, all
real uv knobs that leave `uv run`'s own exit status at 0 — `which` silently falls
through to whatever `ruff` is first on the OUTER host PATH, and `[ -x ]` passes on that
impostor. MEASURED: with `ruff` removed from `py/.venv/bin` and a host impostor earlier
on `PATH`, the bare `shutil.which` resolver printed the impostor's path at exit 0 —
exactly the "strictness is a property of the host" failure SMA-525 refused, now green.
The fix requires the resolved path to live under `sys.prefix`, which under `uv run
--project py` IS `py/.venv` (measured) — a same-process check no outer-`PATH`
manipulation can spoof. Re-run with the fix in place, the same setup now exits 1 from
the `-c` program (routed to `die_infra`, rc 2) instead of printing the impostor's path.

## Corpus derivation: `git ls-files`, not a glob ruff walks itself

The file list `run_check` passes to `ruff check` is derived once, with `git ls-files`,
and passed as an explicit argument list. It is not asserted against ruff's own
discovery after the fact — the list **is** what ruff is given, so the two cannot drift
apart.

**The pathspec fact, measured across all three forms.** Git's default pathspec matching
does not set `FNM_PATHNAME`, so `*` spans `/` on its own:

- `'ci/*.py'` **alone, with no `:(glob)` magic at all**, already matches every depth —
  `*` spans `/`, so it reaches a nested file like `ci/pyo3-stub/check.py` just as well
  as a top-level one.
- `':(glob)ci/**/*.py'` **alone** also matches every depth, for a different reason:
  `**/` matches zero directories too, so the pattern still reaches a top-level file.
- `'ci/**/*.py'` **with no magic at all** is the one broken form: it misses a top-level
  `ci/foo.py`, because the literal `/` after `ci` has nothing left to match once there
  is no further `/` before the last path component.

So the two pathspecs `run.sh` passes to `git ls-files` — `':(glob)ci/**/*.py'` and
`'ci/*.py'` — are **mutually redundant**: either one alone already covers the whole
corpus. Both are kept anyway because the explicit `':(glob)ci/**/*.py'` form documents
the nested-file intent that `'ci/*.py'` alone does not make obvious to a reader. What
the self-test's `ci/top.py` row actually guards against is not a dropped `:(glob)` —
either pathspec alone already covers a top-level file — but a *reduction* of the pair
down to the bare, unmagicked `'ci/**/*.py'`, which is the likeliest simplification of a
two-pathspec line that looks redundant, and the one form that is actually broken.

**The corpus floor.** `run_check` refuses to proceed if the derived file list has fewer
than 10 entries — the tracked count at the time this gate was written. This is what
stops a moved or renamed `ci/` directory from silently emptying the gate: `git
ls-files` returning zero rows is not a lint pass, it is the corpus collapsing, and
`repo:input-liveness` cannot see this (`ci/affected-graph/task_inputs.py` only proves
that *declared* Moon task inputs are live — it has no view of what a gate's own script
derives at runtime). Deleting a single `ci/**/*.py` file is a legitimate change that can
still trip this floor, so the die message names the fix (lower `CORPUS_FLOOR` to match
the new, smaller, legitimate corpus) rather than only describing the symptom.

**A `git` failure while deriving the corpus is an infrastructure failure, not a lint
verdict (whole-branch review, M4).** The corpus used to be read with `mapfile -t files <
<(ruff_corpus "$root")` — a process substitution, whose exit status bash discards. A
`git ls-files` failure inside `ruff_corpus` (run outside any git repository, say) would
silently read as zero lines and fall straight into the floor check, reporting **rc 1**
("the repo is wrong") for what is actually an environmental fault, in violation of the
exit-code contract this gate documents for itself. `run_check` now captures
`ruff_corpus`'s output through a plain command substitution and checks its exit status
explicitly, routing a `git` failure to `die_infra` (**rc 2**) instead.

## Why the negative control never calls `uv` (PR 206)

An earlier version of `negative_control` built its fixture with `py/` and `rs/` pulled
in by **absolute symlink** and `.prototools` copied alongside them, then re-executed a
**copy of this script** inside the fixture (the same copy-and-invoke trick `self_test`
still uses for its own empty-corpus check — see the `REPO_ROOT` section above). That
copy recomputed `REPO_ROOT` as the fixture directory and called `resolve_ruff()` there,
which runs `uv run --locked --project py …` — so the control ran a **second, real** `uv`
invocation, reached through the symlink from a directory outside the actual repo.

`uv` resolves a project by the symlink's *target*, not its location, so that second
call landed on the exact same `py/.venv` every other `py` Moon task depends on — not a
throwaway copy. A lone run of this gate never showed a problem, because nothing else
touched the venv at the same moment. Under a concurrent `moon ci`, it did: the extra
`uv run` from inside the fixture raced `contracts:generate`, `py:typecheck`, `py:test`,
`paigasus-kernel-py:test` and `repo:release-parity-py`, and lost — those tasks then
failed with `ModuleNotFoundError`, `Failed to spawn: basedpyright`/`pytest`, and
`semantic-release: cannot execute`, all traceable to a `uv`-driven reinstall
(`Uninstalled 9 packages`) landing mid-run. This reproduced only in CI, under real
concurrency; it does not reproduce running the gate alone (measured, PR 206).

The fix removes `uv` from the fixture entirely, rather than trying to make its second
invocation safe. `resolve_ruff` is now called exactly **once**, from the real repo root,
before the fixture is built — the same one call `run_check` already makes. The fixture
itself no longer needs `py/`, `rs/`, or `.prototools`, since nothing under it ever runs
`uv`, or indeed any code at all: `negative_control` derives the fixture's corpus with
`ruff_corpus` (pure `git -C`, no uv) and invokes the already-resolved `ruff` binary
**directly** against those files, with `--config` pointing at the real
`py/pyproject.toml` by absolute path. There is no second `REPO_ROOT` to resolve and
nothing left to race.

One consequence of invoking the binary directly: `ruff_corpus` returns paths relative to
the fixture directory (`git -C`'s own convention), so the invocation runs with
`CWD=$tmp`. Skipping that `cd` doesn't make the control silently pass — ruff still exits
**rc 1** — but for the wrong reason: `E902` (file not found), not the planted `RUF005`.
Since that's the same exit code a real lint violation produces, a control that dropped
the `cd` would report "passed" without ever having linted the file it planted (measured
while fixing this).

`negative_control` still builds its fixture with a plain `mktemp -d` — not nested under
`$REPO_ROOT`, and not off in a fixed named path either — then initializes it as its own
git repo, copies in the root `.gitignore`, and plants a `RUF005` violation
(`ci/probe/violation.py`). That part of the design is unchanged and unrelated to the
`uv` fix: `.moon/workspace.yml`'s `hasher.ignorePatterns` is a fixed, short list, not
`.gitignore`-aware, so a concurrent `moon ci` task with `inputs: ['**/*']`
(`repo:actionlint`, `repo:input-liveness`) could hash-walk a live in-tree fixture
mid-run, and a `SIGKILL` before the fixture's `EXIT` trap fires would leave cruft in the
real tree instead of in `$TMPDIR`. The root `.gitignore` copy is kept for the same
reason it was added originally — a defensive floor, so a plain `git add -A` would still
exclude a vendored `.venv` tree the way the real repo does, if the fixture's
construction ever grows one again — even though today's minimal fixture contains
nothing to exclude.

The control's own cleanup uses an `EXIT` trap, not a `RETURN` trap: its failure branch
calls `exit 1` directly, which ends the process without ever returning from the
`negative_control` function, and a `RETURN` trap does not fire on that path (measured —
an earlier version of this script left a stray fixture directory on disk whenever the
control itself failed). The trap references a variable set at file scope (`tmp=""` near
the top of the script), not a function-local one, because by the time an `EXIT` trap
actually runs — after the whole script's `case` dispatch has completed — a `local`
binding from inside the function is already out of scope, and referencing it under
`set -euo pipefail` would itself be an unbound-variable error.

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

## Further limitations (whole-branch review)

**L11 — `check_self_scheduled_coverage` reads only a task's `script:` key.** It derives
its "must have a `SELF_SCHEDULED_GATES` entry" set from `_scripts()`, which reads the
resolved `script:` block of a `repo:*` moon task. A future self-scheduled gate written
as `command:` + `args:` instead of a `script:` block would be invisible to this check —
it would carry a `--self-test`/`--negative-control`-shaped invocation that
`check_self_scheduled_coverage` never sees, and could ship with no
`SELF_SCHEDULED_GATES` entry undetected. Not reachable today: every `repo:*` gate in
this repo is written as a `script:` block.

**L12 — the pinned corpus-derivation line can go dead without reddening.**
`RUFF_SH_CALL_SITES` pins the `git -C "$root" ls-files -- ...` line *inside*
`ruff_corpus()`, but `run_check`'s own *call* to `ruff_corpus` is not itself pinned. A
rewrite of `run_check` that stopped calling `ruff_corpus` altogether (inlining a
different derivation, say) would leave the pinned line intact in a function nothing
calls, and the pin would keep passing while asserting nothing about what `run_check`
actually does. This is the same residual shape `ci/release-parity/README.md`'s L5
records for its own pinned-but-orphanable line.

**L13 — RESOLVED by PR 206.** `negative_control()` used to re-enter the whole gate
(`run_check`, via a copy of this script), so its rc-1 check could not distinguish "ruff
caught the planted violation" from "the corpus floor (`CORPUS_FLOOR`) tripped for an
unrelated reason" — both exit rc 1. Since PR 206, `negative_control()` invokes the
already-resolved `ruff` binary directly and never goes through `run_check` or its floor
check at all, so that ambiguity no longer exists structurally: an empty fixture corpus
now makes `ruff check` itself exit rc 2 (`error: a value is required for '[FILES]...'`,
measured), which the control's `!= 1` guard already treats as a failure, not a pass.
