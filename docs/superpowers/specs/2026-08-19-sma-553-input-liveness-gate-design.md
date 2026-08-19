# SMA-553 — Assert every `repo:*` task's inputs still match a tracked file

**Status:** draft
**Linear:** [SMA-553](https://linear.app/smaschek/issue/SMA-553/repo-assert-every-repo-tasks-inputs-still-match-a-file-a-gate-can-be)
**Related:** SMA-541 (limitation L3, which filed this), SMA-525 (`repo:actionlint`, whose `inputs:
['**/*']` comment reasons about exactly this hazard), SMA-524 / SMA-534 / SMA-546 (the sibling
assertions in `ci/affected-graph/`), SMA-378 (the `uv_build` license gotcha this spec's §7 finding
belongs to)

## 1. Problem

SMA-541 proves a `repo:*` gate is **wired into CI**: present in `ci.yml`'s `T=(…)` array, resolving
to a CI-eligible task, mirrored in CLAUDE.md. This is the layer below — a gate that is wired,
resolvable, and **never fires**, because its `inputs` no longer match anything.

`repo:promtool` declares `inputs: ['ops/observability/prometheus/**/*']`. Move or rename `ops/` and
that glob matches zero files, so Moon never schedules the task again. `moon ci` stays green, the
target stays in `T`, SMA-541's C1-C5 all pass — and the gate has silently stopped existing. Same
failure class SMA-541 was filed for, one layer down.

The surface is wide because `repo` owns the whole tree, so almost every `repo:*` gate carries a
narrow hand-written glob precisely to avoid running on every change: `osv`, `machete`, `promtool`,
`observability-drift`, `nats-permissions`, `redis-connect-single-site`,
`iam-docker-policy-single-site`, `parity-corpus-drift`, `next-env-drift`, `release-parity*`,
`publish-metadata`, `wasm-getrandom-free`.

Nothing checks this today, and SMA-541's design doc already corrects an earlier draft that claimed
otherwise: `cargo_moon_parity.py`'s A4 iterates Cargo crates, A5 derives FFI tasks from command
markers, and `lockfile->all-lint` is a Rust-task case. None looks at a `repo:*` task's inputs.

## 2. Evidence

Measured on 2026-08-19 against the pinned moon **2.3.2**, before any design decisions were made.
Everything here was observed, not reasoned about. E1, E2, E6 and E8 each contradict a premise in the
issue text or in an obvious first design; they are marked.

**E1 — Moon reports inputs VERBATIM, unresolved.** The finding the whole design turns on. A throwaway
`repo` task declaring inputs that do not exist:

```yaml
  ghost-probe:
    inputs:
      - 'nonexistent-dir/**/*'
      - 'nonexistent-file.txt'
```

reports them back unchanged:

```
inputFiles: {'nonexistent-file.txt': {}}
inputGlobs: ['nonexistent-dir/**/*', '.moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}']
```

`moon query projects`' `inputFiles` / `inputGlobs` are **normalised**, not **resolved**: a
path-keyed dict of what the author declared, with the leading `/` of a workspace-relative entry
stripped and a bare directory expanded (E6). Moon never touches the filesystem to produce them. So
Moon's own output can never tell you a gate has gone dead — *(this corrects the issue text's step 1,
which says "Read `moon query projects`' **resolved** `inputFiles` / `inputGlobs`")*. The gate must
match against tracked files itself.

`moon task repo:promtool` is no help either: it pretty-prints the same four declared patterns.

**E2 — every task carries a Moon-injected glob, so a resolved input set is NEVER empty** *(this
makes the issue's AC #2 unreachable as literally worded)*. All **119** tasks across all **28**
projects carry `.moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}`. A task declaring literally `inputs: []`
still reports it:

```
has inputFiles key: False
inputGlobs: ['.moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}']
raw inputs: [{'glob': '/.moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}', 'cache': True}]
```

"Fail if a task's resolved input set is empty" therefore asserts nothing until that injected glob is
subtracted. With the subtraction it becomes a real check: a `repo` task with no authored inputs keys
only on `.moon/` config edits, which is a broken gate.

**E3 — `repo` is 100% clean; nothing else is.** Matching every declared pattern against
`git ls-files -- ":(glob)…"` across the whole graph:

| | count |
|---|---|
| dead globs on `repo:*` | **0** |
| dead globs on the other 27 projects | **98** |
| declared file inputs not tracked | **4** (all `py`, see §7) |
| tasks with no authored inputs at all | **2** (`ts:check-config-only`, `ts:commitlint`) |

The 98 are essentially all *speculative convention globs* inherited from
`.moon/tasks/{rust,typescript,python}.yml` — `**/*_test.rs`, `tests/**/*`, `**/*.spec.ts`,
`ts/src/**/*`, `py/src/**/*`. `paigasus-kernel-rs` has no `tests/` directory, so its inherited
`tests/**/*` is legitimately dead. A whole-graph check would red on day one, which is SMA-541's E4
finding reproduced one layer down.

**E4 — `git ls-files -- ":(glob)P"` agrees with wax on every pattern in use, except braces.**

| pattern | tracked matches |
|---|---|
| `rs/**/Cargo.toml` | 14 — **including `rs/Cargo.toml`**, so `**` spans zero segments as wax does |
| `**/*` | 714 — every tracked file, including the 9 at top level and all dotfiles |
| `ops/observability/prometheus/**/*` | 7 |
| `rs/crates/*/*/moon.yml` | 13 |
| `.moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}` | **0** — git pathspec has no brace expansion |

714 is also the repo's total tracked-file count, so `**/*` is exactly the whole tree. Graph-wide
there are **zero** negated (`!`-prefixed) globs and **zero** brace globs other than the injected one.

**E5 — `repo` inherits no task config, so its injected input set is a single known string.**
`moon query projects --id repo` reports `inherited.configs: []` and `layers: {}`. Every other stack
inherits one: `paigasus-kernel-rs:lint` carries `.moon/tasks/rust.yml` as an additional global
`inputFile`, and `paigasus-ml-py:build` carries `globalInputs: [{"file":
"/.moon/tasks/python-project.yml"}]`. Moon's *merged* task object records no per-input provenance —
`inherited.configs[…].tasks[…].inputs` holds the unmerged, project-relative, unexpanded form
(`{"file": "pyproject.toml"}`, `@group(sources)`), so recovering "authored vs inherited" graph-wide
means re-implementing file-group expansion and path resolution.

**E6 — a bare directory input arrives as a glob** *(so I2 needs no directory-prefix fallback)*.
`inputs: ['ops/nats', 'ops/observability/prometheus/rules']` reports:

```
raw:        [{'file': 'ops/nats'}, {'file': 'ops/observability/prometheus/rules'}, …]
inputFiles: (absent)
inputGlobs: ['.moon/*.{…}', 'ops/nats/**/*', 'ops/observability/prometheus/rules/**/*']
```

Moon classifies the raw entry as a `file` but expands it into `inputGlobs` as `dir/**/*`, leaving
`inputFiles` empty.

**E7 — `moon query`'s `--id` filter is an unanchored regex.** `--id epo` returns `repo`; `--id
paigasus-kernel` returns `paigasus-kernel-parity-rs`, `paigasus-kernel-py`, `paigasus-kernel-rs`,
`paigasus-kernel-ts`. SMA-541's D8 warned about this for `--project`; it is measured true for `--id`
too. `moon query tasks` sidesteps it entirely — its output is `d['tasks'][<exact project id>]`.

**E8 — an absent `inputFiles` key is LEGITIMATE here** *(this corrects the issue text's step 1,
which says "Treat an absent key as a violation, never a skip — the convention A4 already follows")*.
Seven tasks graph-wide have no file inputs because every input they declare is a glob — five of them
on `repo`: `actionlint`, `iam-docker-policy-single-site`, `machete`, `observability-drift`,
`redis-connect-single-site`. Applying A4's rule verbatim would red five clean gates on day one. A4
can hold that rule because it asserts specific named files must be present on a `lint` task; this
gate iterates whatever is declared, so absence means "no file inputs", not "Moon told us nothing".

**E9 — timings**, on this machine, warm:

| | wall |
|---|---|
| `ci/affected-graph/run.sh` (real suite) | 29.3s |
| `ci/affected-graph/run.sh --negative-control` | 5.2s |
| ⇒ `repo:affected-smoke` | ~35s + Moon's per-task floor |
| `moon query projects` (one call) | ~2.5s |
| `repo:actionlint` — `inputs: ['**/*']` + script (SMA-525's figure) | ~11.6s |
| `repo:promtool` — Moon's narrow per-task floor (SMA-525's figure) | ~8.7s |

**E10 — `.moon/workspace.yml` states this issue's invariant in prose, unguarded.** Lines 41-43,
verbatim:

> AND DO NOT ADD ONE EITHER: no task may declare an `inputs:` path under these trees. A path
> excluded here contributes nothing to any cache key, so the task that names it would never be
> invalidated by a change to it — a permanently stale cached pass, with nothing red to notice.

`ignorePatterns` is `['**/node_modules/**', '**/target/**', '**/.venv/**']`, and the block above it
records the compliance check as "verified" by hand. That is SMA-553's failure class, written down,
with no gate behind it.

## 3. Design decisions

**D1 — the check lives in a new `ci/affected-graph/task_inputs.py`.** A third file rather than an
extension of `cargo_moon_parity.py` (29k, Cargo-centric, though its A4 is precedent for an inputs
assertion living there) or `ci_targets.py` (51k). Own concern, own fixtures, own README bullet,
matching SMA-541 D1's reasoning for `ci_targets.py`. It keeps `ci/affected-graph/`'s established
shape: pure functions with fixture tables, a `--self-test` flag, 0/1/2 exit codes.

**D2 — but it is scheduled by its OWN `repo:input-liveness` task, not by `run.sh`.** This is forced
by E1 combined with `repo:affected-smoke`'s narrow inputs. The gate's verdict depends on the
**entire tracked file tree** — a glob dies when files move — while `affected-smoke` keys on
`moon.yml`, `ci/affected-graph/**/*`, `.moon/**/*`, the manifests, `CLAUDE.md` and `.prototools`.
Rename `ops/` and `repo:promtool`'s glob dies, but nothing in that list changed, so `affected-smoke`
serves a **cached PASS** and the headline acceptance criterion silently does not hold. That is the
same vacuity trap SMA-541's D9 had to add `CLAUDE.md` to close.

The only honest input for this check is `inputs: ['**/*']` — the conclusion `repo:actionlint`
already reached, and which the issue cites. The two ways to get there were measured (E9): broaden
`affected-smoke` to `**/*`, making its ~35s suite run on **every** PR; or a standalone task at
~11.6s. The standalone task wins on cost and, being independently scheduled, is also the vehicle
SMA-541's L6 names for eventually closing the gate-inside-the-thing-it-guards hole. Cost: the task
must be added to `T` and to CLAUDE.md's marker region — which is SMA-541's own rule, so this change
self-exercises C1 and C3.

**D3 — the negative control runs FIRST, inside the task's own script**, mirroring
`repo:affected-smoke` and `repo:publish-metadata`. Without it CI runs only the real check, so the
self-test that proves the assertions can FIRE is never executed and a rotted control ships green —
the failure SMA-526 hit. Moon does not enable errexit for `script:` blocks, so `set -euo pipefail`
is explicit, as every other multi-line `repo:*` script in this file already documents.

**D4 — "authored inputs" means the declared set minus Moon's injected glob.** Forced by E2: without
the subtraction, I3 asserts nothing. E5 is why this is safe for `repo` and would not be graph-wide —
`repo` inherits no task config, so exactly one string is subtracted, and I5 asserts that string is
present on **every** repo task so a Moon change that stops injecting it, or renames it, fails loudly
instead of silently changing what "authored" means.

**D5 — the matcher is `git ls-files -- ":(glob)P"`, and "tracked" is the deliberate predicate.**
Reuses `pattern_verdict` (`ci/actionlint/run.sh:919`) and its two recorded portability lessons: do
not anchor on `^\./` (GNU grep emits the prefix, ugrep strips it), and pass paths explicitly rather
than `.`. E4 shows it agrees with wax on every pattern in use.

Tracked rather than on-disk is what makes E10's invariant real: `node_modules`, `target` and `.venv`
are gitignored, so a glob confined to one matches no tracked file, is dead, and reds. It also
correctly rejects an input under a tree that contributes nothing to any cache key.

**D6 — the pattern validator is default-deny, and a rejected pattern is rc 1, not a skip.** Before
any pattern reaches git it is classified, in `pattern_verdict`'s vocabulary:

- `{` or `}` → **rejected-braces**, with its own message. git pathspec has no brace expansion (E4),
  and expanding braces here is the hand-rolled parsing `ci/actionlint/run.sh:265` explicitly warns
  against. Zero authored brace globs exist.
- anything outside `[A-Za-z0-9._/*-]` → **rejected-charset**. Doubles as the pathspec-injection
  guard: a pattern starting with `:` would otherwise be read by git as pathspec magic, and `--` plus
  quoting is necessary but not sufficient.
- a `**` that is not a whole path component → **rejected-globstar**.
- a `.`, `..` or empty path segment → **rejected-dotty**. git normalises these away when resolving a
  pathspec; whether Moon does is unmeasured, so the gate refuses to guess.

A skip would be the silent-hole failure this whole issue is about. Failing means the gate says what
it will not evaluate, and the author extends the validator deliberately — D10's stance in SMA-541.

**D7 — two live-fire canaries run on EVERY real invocation, not only in `--self-test`.** A known-dead
pattern must verdict `dead`, and a known-live one must verdict `ok`. This is the one failure the
fixture table cannot catch: a matcher stuck returning "live" — a `git` invocation silently changing
behaviour, a pathspec form git stops honouring — passes I1, I2 and I4 vacuously while every check
still prints PASS. It costs one extra `git` call. `ci/actionlint/run.sh`'s
`expect_pattern 'rz/**' 'dead'` asserts the same thing against the real tree; this promotes it from
the self-test into the production path.

**D8 — one `moon query tasks` call, keyed by exact project id.** Not `moon query projects --id repo`
(E7: unanchored regex — a future project named `paigasus-repo-ts` would silently join the set).
`moon query tasks` returns `d['tasks']['repo']`, an exact key lookup with no filter to get wrong, and
carries everything needed: `inputFiles`, `inputGlobs`, `options`, and the resolved `script` that D10
uses. It is also the call `ci_targets.py` already makes, so the two gates read the same shape.

E1, E2 and E6 were measured through `moon query projects` because that is the command the issue text
names. The **task object is identical under both** — verified: `moon query tasks`' entry for
`repo:affected-smoke` carries the same twelve keys (`command`, `description`, `id`, `inputFiles`,
`inputGlobs`, `inputs`, `options`, `script`, `state`, `target`, `toolchains`, `type`). So those three
findings transfer unchanged; only the envelope around the task object differs.

**D9 — `runInCI: false` tasks ARE checked.** SMA-541's C1 excludes `install-hooks` because it asks
"does CI run this". I1-I3 ask "can this task ever be scheduled at all", which matters for a local
`moon run` too. Its inputs (`lefthook.yml`, `.lefthook/**/*`) are live today, so this costs nothing
and closes a hole by default rather than by luck.

**D10 — the self-guard extends SMA-541's C4 rather than adding a circular one.** D2 puts the gate in
its own task, so its **existence** is already guarded — SMA-541's C1 is strict equality between `T`'s
repo entries and the repo task set, so deleting `repo:input-liveness` from `moon.yml` while leaving
`:input-liveness` in `T` reds C1, and deleting both reds C3 against CLAUDE.md. What remains unguarded
is the task's **script**: rewriting it to drop the `--self-test` line, or to swallow a failure,
leaves everything green.

So `ci_targets.py`'s `RUN_SH_CALL_SITES` grows from two `run.sh` call sites to also require both
invocations in `repo:input-liveness`'s resolved `script` (D8 already fetches it), each matched **with
its propagation** — the lesson that check already records, where matching a prefix alone let
`--self-test || true` look identical to a wired call site. C4 lives in `repo:affected-smoke`; the
gate lives in its own task. Neither can suppress the other, so this has none of L6's circularity.

**D11 — `ALLOW_DEAD_INPUT` ships empty, with a required non-empty reason.** `repo` is measured 100%
clean (E3), so unlike `T_EXEMPT` there is not even a hypothetical entry on day one. It mirrors
`ALLOW_NO_CARGO_BACKING` (`cargo_moon_parity.py:53-61`): a `{(task, pattern): reason}` map where an
empty reason is itself an assertion failure. Two staleness rules, both from SMA-541 D5's
leftover-exemption lesson: an entry naming a task that does not exist is rc 1, and so is one naming a
pattern that task does not declare. A typo is loud either way — the real pattern shows up as a
violation — but a leftover entry exempts nothing, forever, and is silent.

**D12 — exit codes 0/1/2, with rc 2 reserved for genuine tool failure**, exactly as SMA-541 D2.
`moon` or `git` failing, output that will not parse as JSON, or a shape lacking a key the gate needs
→ rc 2. Every authorial mistake — a dead glob, an untracked file, an empty authored input set, a
stale allowlist entry — is rc 1 with a message naming what to edit. Unlike `ci_targets.py`, an rc 2
here aborts only this task, not a suite of eight other assertions, because D2 gave the gate its own
task; the split is kept anyway so the two siblings triage identically.

## 4. Components

### `ci/affected-graph/task_inputs.py`

**Inputs** — two:

| # | Source | Used for |
|---|---|---|
| 1 | `moon query tasks` (one call, D8) | `repo`'s `{task → inputFiles, inputGlobs, options}` |
| 2 | `git ls-files` / `git ls-files -- ":(glob)P"` | the tracked set, and per-pattern liveness |

**Checks:**

- **I1 — no dead glob.** Every authored glob must match ≥1 tracked file. Failure names the task and
  the pattern, and says a rename or move is the likely cause.
- **I2 — no dead file input.** Every authored `inputFiles` entry must be tracked. Separate verdict
  from I1 (`not-exact` vs `dead`), following `pattern_verdict`'s vocabulary, because the fix differs.
- **I3 — no task without authored inputs.** After the D4 subtraction, each task must declare ≥1.
- **I4 — every authored pattern is evaluable.** Any `rejected-*` verdict from D6 is a failure naming
  the pattern and the specific reason.
- **I5 — anti-vacuity floors.** rc 1 unless noted:
  - `d['tasks']['repo']` absent or empty → **rc 2** (shape change or a filter that stopped matching)
  - the tracked-file set empty → **rc 2** (git ran but told us nothing)
  - a repo task missing the injected glob → **rc 2**, because D4's subtraction would silently change
    meaning
  - `REQUIRED_TASKS = ('affected-smoke', 'input-liveness', 'promtool', 'publish-metadata')` must all
    be present — the `REQUIRED_FFI_TASKS` precedent (`cargo_moon_parity.py:95-103`). It names
    **itself**, so the gate's own task vanishing is red
  - the two D7 canaries
  - every `ALLOW_DEAD_INPUT` entry carries a non-empty reason, names an existing task, and names a
    pattern that task declares (D11)

### `moon.yml`

A new `repo:input-liveness` task. `inputs: ['**/*']` per D2, with a comment giving the reason in the
manner of `repo:actionlint`'s. Script per D3.

### `ci/affected-graph/ci_targets.py`

`RUN_SH_CALL_SITES` extended per D10 from two entries to four. Its existing fixture row ("`run.sh`
text missing either call site → C4 red") is extended to cover the two new entries, and the constant
is renamed to reflect that it no longer describes `run.sh` alone.

### `.github/workflows/ci.yml`

`:input-liveness` appended to the single-line `T=(…)` array at line 215.

### `CLAUDE.md`

`:input-liveness` added inside the `ci-targets` marker region, and a gotcha line recording that a
`repo:*` task's inputs are now asserted live — so moving a directory a gate keys on reds
`repo:input-liveness` rather than silently switching that gate off.

### `ci/affected-graph/README.md`

A bullet describing I1-I5, the `ALLOW_DEAD_INPUT` contract, and the measured cost.

## 5. Testing

`--self-test` drives the **pure functions** against in-memory fixtures, so no verdict depends on the
tree happening to be aligned. `classify(pattern)` and `check(tasks, matcher)` take data and return
rows; the real run passes a git-backed matcher, the self-test a stub. Every row below names a fixture
that **exists** in `self_test()` — a documented-but-absent control is the same drift class this issue
exists to close.

| Fixture | Expected |
|---|---|
| a task with a glob matching nothing | I1 **red**, naming task + pattern |
| a task with an untracked file input | I2 **red**, naming task + path |
| a task whose only glob is the injected one | I3 **red** |
| `actionlint`'s `**/*` | **green** — AC #4, asserted not assumed |
| a bare directory arriving as `dir/**/*` (E6 shape) | **green** |
| a brace / charset / non-component-`**` / dotty pattern | I4 **red**, one row each, distinct messages |
| an allowlisted `(task, pattern)` with a reason | **green** |
| an allowlist entry with an empty reason | **red** |
| an allowlist entry naming no repo task | **red** — the exemption outlived its task |
| an allowlist entry naming a pattern the task does not declare | **red** |
| a task set missing a `REQUIRED_TASKS` member | **red** |
| a stub matcher stuck returning "live" | **red** via the dead canary (D7) |
| an empty `d['tasks']['repo']` | **rc 2** |
| an empty tracked set | **rc 2** |
| a repo task missing the injected glob | **rc 2** |
| `git` or `moon` raising | **rc 2** |
| everything aligned | **green** — catches a permanently-red harness |

The three rc-2 raises are reachable from fixtures because the shape rules live in pure functions that
the subprocess wrappers call, the same split `cargo_moon_parity.py` uses to fixture its own infra
raise.

Beyond the table, verification is by **mutation against the real tree**, run and recorded rather than
assumed:

1. point `repo:promtool`'s glob at a nonexistent directory → the gate names the task and the glob
2. `git mv ops/observability ops/obs` → the same, for `promtool` **and** `observability-drift`
3. set a `repo` task to `inputs: []` → I3 fires
4. add an `ALLOW_DEAD_INPUT` entry with an empty reason → red
5. delete the `--self-test` line from the task's script → SMA-541's C4 fires
6. remove `:input-liveness` from `T` → SMA-541's C1 fires; remove it from CLAUDE.md → C3 fires
7. the unmutated tree is green

The wall-clock cost of the new task is measured and recorded in the README, since D2 chose it partly
on cost grounds.

## 6. Limitations

- **L1 — `repo` only.** E3 measured 98 dead globs across the other 27 projects, essentially all
  speculative convention globs inherited from `.moon/tasks/{rust,typescript,python}.yml`. They are
  not defects: `paigasus-kernel-rs` legitimately has no `tests/` directory. Covering them needs
  authored-vs-inherited provenance, which E5 shows Moon's merged output does not record — recovering
  it means re-implementing file-group expansion and project-relative path resolution, which is the
  hand-rolled parsing D6 refuses on a much smaller surface. Widening the scope is not the fix; if the
  convention globs are ever pruned from the templates, the gate can widen for free.
- **L2 — liveness, not sufficiency.** A glob matching one file when it should match twenty is
  invisible. This gate asserts a task *can* be scheduled, not that its inputs are complete. The
  complementary assertion — "this edit makes that task affected" — is what `run_task_case` in
  `ci/affected-graph/run.sh` does, case by case.
- **L3 — untracked-but-real files read as dead.** Correct for cache-key purposes (D5), but a
  generated, gitignored input would be reported as a violation. None exists on `repo` today.
- **L4 — E10's coverage is incidental, not asserted.** The `hasher.ignorePatterns` invariant holds
  only because `node_modules`, `target` and `.venv` are gitignored. Removing one from `.gitignore`
  would silently end that coverage with nothing red. Asserting it directly means reading
  `.moon/workspace.yml`, which is YAML parsing this file avoids.
- **L5 — bare-directory handling rests on E6.** Because Moon expands `inputs: ['ops/nats']` into a
  glob, I2 needs no directory-prefix fallback. A future Moon that stopped doing that would make a
  directory input read as an untracked file — a false red, not a false green, and no floor catches
  it. Recorded rather than pre-solved, because unreachable defensive code is its own drift.
- **L6 — a `repo` gate defined outside the root `moon.yml`** — a future `ci/newgate/moon.yml`
  declaring its own project — is invisible to this gate, exactly as it is to SMA-541's C1 (its L9).
  The fix, if `ci/` ever grows a second project, is the same: widen the partition to an explicit list
  of gate-owning project ids.
- **L7 — the gate cannot see a task switched off with `internal: true`.** Per SMA-541's E3,
  `moon query tasks` omits an internal task entirely, so I1-I3 never consider it. `REQUIRED_TASKS`
  catches it for the four named tasks only.
- **L8 — break-glass.** The fix path for a red is always "fix the input, or record an
  `ALLOW_DEAD_INPUT` entry with a reason". There is no warn-only mode by design.

## 7. Finding outside this issue's scope

`paigasus-ml-py:build` and `paigasus-workflows-py:build` declare `README.md` and `LICENSE` as inputs,
inherited from `.moon/tasks/python-project.yml`. **None of the four files exists** — not gitignored,
absent (`ls` shows only `moon.yml`, `pyproject.toml`, `src/`). Both packages are PyPI-bound with
`uv_build`, which is the SMA-378 gotcha: `uv_build` does not auto-glob license files, so a wheel
built today would ship no license text.

Real, latent (no release workflow is wired — the `python-semantic-release` config is dormant), and in
a different workspace than this issue's subject. A follow-up Linear issue is filed rather than fixed
here, to keep this PR's diff to the gate.

## 8. Acceptance criteria

| Issue AC | Covered by |
|---|---|
| Renaming a directory a `repo:*` task's `inputs` depend on fails the gate, naming the task and the dead glob | I1, made reachable by D2's `inputs: ['**/*']`; mutation 2 in §5 |
| A task whose resolved input set is empty fails the gate | I3 — reinterpreted per E2 as the **authored** set, since the resolved set is never empty (D4) |
| The check carries its own control proving it can fire, verified by mutation rather than asserted | `--self-test` run first by the task itself (D3), plus the seven §5 mutations, plus D7's canaries on every real run |
| `repo:actionlint`'s `inputs: ['**/*']` does not trip it | E4 (714 tracked matches) + its dedicated fixture row |
| Whatever remains uncovered is recorded honestly, in the manner of SMA-541's §6 | §6, L1-L8 |
