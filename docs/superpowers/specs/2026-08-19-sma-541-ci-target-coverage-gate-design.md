# SMA-541 — Assert every `repo:*` gate is wired into `ci.yml`'s `moon ci` target array

**Status:** drafted 2026-08-19
**Linear:** [SMA-541](https://linear.app/smaschek/issue/SMA-541/repo-assert-every-repo-gate-is-actually-wired-into-ciymls-moon-ci)
**Related:** SMA-525 (limitation L6, which filed this), SMA-524 / SMA-534 / SMA-546 (the sibling
assertions in `ci/affected-graph/`), SMA-542 (guards the actionlint gate's self-test invocations —
the same durability concern one level down)

## 1. Problem

`.github/workflows/ci.yml:215` runs `moon ci` over a **hand-written** target array:

```bash
T=(:build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke …)
```

Nothing asserts that array is complete. A future `repo:*` gate can be added to `moon.yml`, be
perfectly correct, pass locally via `moon run repo:<name>`, and **never run in CI** because nobody
appended it to `T`. There is no red check. The gate simply does not exist.

That is the same silent-omission class as SMA-525 itself, one level up: a guard that is present in
the tree, believed to be running, and is not.

`ci/affected-graph/run.sh` already parses `ci.yml`, but only to assert every `moon ci` invocation
carries `--include-relations` (`assert_include_relations`). It never looks at the array's contents.

CLAUDE.md's "run the full graph like CI does" procedure (`CLAUDE.md:64-68`) enumerates the same
targets by hand a second time and can drift from `T` in either direction — so the documented way to
reproduce CI locally can quietly stop reproducing CI.

## 2. Evidence

Measured on 2026-08-19 against the pinned moon **2.3.2**, before any design decisions were made.
Everything in this section was observed, not reasoned about.

**E1 — an unresolvable target is not an error.** This is the finding the whole design turns on:

```
$ moon ci :bogus-target --base origin/main --include-relations ; echo $?
Requested targets: 1
        :bogus-target
Resolved targets: 0
0

$ moon run :bogus-target ; echo $?
CAUTION  No tasks found. Unable to execute action pipeline. For targets :bogus-target.
1
```

`moon ci` exits **0** having resolved nothing. So a typo'd or renamed entry in `T` is a silent
no-op on every PR. (The `moon run "${T[@]}"` fallback branch at `ci.yml:222` *would* fail — but it
runs only on an initial push with no usable base, which is not a path anyone exercises.)

**E2 — the task inventory.** `moon query tasks --project repo` emits JSON natively; `--json` is
**not** a valid flag on moon 2.3.2 (`error: unexpected argument '--json' found`). The `repo`
project has **18** tasks. `install-hooks` is the only one with `runInCI: false`. The other 17 are
all present in `T` today, alongside 6 targets owned by other projects (`:build :test :lint :fmt
:typecheck :breaking`). `T` therefore holds 23 entries and is currently correct — this change adds
no cleanup wave.

**E3 — no task anywhere in the graph is `internal: true`.** Across every project, the only task
excluded from CI by its own options is `repo:install-hooks`. So the exclusion rule is exactly the
one the issue names, with no second category to reason about.

**E4 — the obvious CLAUDE.md selector is not unique.** CLAUDE.md contains **161** inline-code
spans; **5** begin with `moon ci`; **2** of those also contain `--include-relations`:

| span | is it the full-graph command? |
|---|---|
| `moon ci :build :test :lint :fmt :deny … --base origin/main --include-relations` | yes |
| `moon ci --include-relations` (in the affected-model gotcha) | no |

Selecting on "starts with `moon ci`, contains `--include-relations`" would match both. Adding
"…and contains at least one `:target` token" matches exactly one. So does "contains `--base
origin/main`". Either works today; the design below uses the former and, critically, **requires
the match count to be exactly 1**.

**E5 — `CLAUDE.md` is an input to no Moon task.** Verified against the whole of `moon.yml`. Without
a change there, a CLAUDE.md-only edit leaves `repo:affected-smoke` serving a **cached PASS**, and
the docs assertion below would be real but unreachable — the exact staleness `ci/next-env/run.sh`
and the `parity-corpus-drift` comments already document.

## 3. Design decisions

**D1 — the gate lives in `ci/affected-graph/ci_targets.py`, a Python sibling of
`cargo_moon_parity.py`, invoked from `run.sh`.** Not a bash function inside `run.sh` (three
parsers, one of them a line-wrapped markdown span, plus a fixture table — all grim in bash, in a
file already at 18k), and not a new `repo:*` gate of its own (which would need its own `ci/`
directory and README, would itself have to be added to `T` and CLAUDE.md by its own rule, and would
pay a fresh ~11.6s Moon per-task floor for no gain). `repo:affected-smoke` already reads `ci.yml`
and `moon.yml`, already runs a `--negative-control` pass in CI, and `cargo_moon_parity.py` already
establishes the exact shape — a Python module with an rc 0/1/2 contract and a `--self-test` flag
wired into that control.

**D2 — exit codes: 0 pass, 1 assertion failure, 2 infrastructure error.** The `ci/` convention.
`run.sh`'s `assert_ci_targets` folds rc 1 into `SUITE_RC` and aborts the whole guard on rc 2, so a
broken `moon` is never mistaken for a coverage regression — mirroring `assert_cargo_moon_parity`.

**D3 — forward check (C1) uses `runInCI` as the sole exclusion.** Every `repo` task whose
`options.runInCI` is not `false` must appear in `T` as `:name`. Moon's own field is the canonical
way to say "this is not a CI gate"; introducing a second, script-local exemption list would create
a way to dodge the check that is invisible from `moon.yml`.

**D4 — no escape-hatch allowlist for a CI-eligible task deliberately absent from `T`.** No such
task exists (E2), and a hand-maintained exemption table added preemptively is precisely the
vacuous-gate shape SMA-525 kept finding. If a real case appears, add the allowlist then, with a
required reason string per `cargo_moon_parity.py`'s `ALLOW_NO_CARGO_BACKING` precedent.

**D5 — reverse check (C2) asserts resolvability, not repo-ownership.** Every `:name` in `T` must
match at least one task name somewhere in the task graph. This is what E1 makes worthwhile: a
renamed `repo:promtool` or a typo'd `:afected-smoke` leaves a dead entry that `moon ci` swallows
with exit 0. Deliberately *not* strict equality against the `repo` task set: `T`'s six generic
targets are legitimate and are owned by other projects, and a hardcoded generic-target allowlist
would be one more hand-maintained table to drift.

Note C1 and C2 overlap on a typo (`:afected-smoke` trips both) but are not redundant: C1 alone
misses a task deleted from `moon.yml` whose stale entry survives in `T`, and C2 alone misses a new
gate that was simply never added.

**D6 — docs check (C3) is an ordered, token-for-token mirror of `T`.** Not set equality. The rule
is then trivially stateable ("copy `T`") and trivially fixable, and the doc is a literal mirror
rather than a merely-equivalent set, so a reader can diff the two by eye. Line wrapping is
normalised away before comparing, so reflowing the CLAUDE.md paragraph is free; reordering `T`
means updating the doc in the same commit. Only the `:target` tokens are compared — the trailing
`--base origin/main --include-relations` is not part of the assertion (`assert_include_relations`
already owns the flag question for `ci.yml`).

**D7 — ambiguity is an infrastructure error, never a silent pick.** Given E4, the CLAUDE.md
selector is "an inline-code span that starts with `moon ci`, contains `--include-relations`, and
contains at least one `:target` token", and the gate aborts with rc 2 unless **exactly one** span
matches. Zero matches means the procedure was deleted or reworded; two means someone added a
second full-graph example and must disambiguate. Both are louder than guessing.

**D8 — anti-vacuity controls are part of the gate, not just the self-test.** Each of the following
aborts rc 2 on the *real* run: the `repo` task set comes back empty; `T=(…)` matches zero or more
than one line in `ci.yml`; the CLAUDE.md span match count is not 1. Same "the guard is not guarding
anything" control that `repo:redis-connect-single-site` and `repo:iam-docker-policy-single-site`
already carry, and the reason those two survive refactors of the code they watch.

**D9 — `CLAUDE.md` joins `repo:affected-smoke`'s `inputs`.** Forced by E5. This is the one cost the
design imposes on unrelated work: any CLAUDE.md edit now re-runs that gate (~12s warm through
Moon). Accepted — the alternative is an assertion that cannot fire on the file it asserts about.

**D10 — every entry of `T` must be the `:name` shorthand; a project-scoped entry fails loudly.**
Moon accepts both `:promtool` (all projects) and `repo:promtool` (one project). The parser takes
`:`-prefixed tokens, so a scoped entry would be **silently ignored by both C1 and C2** — the gate
would report green while the array contained something it never examined, which is the exact
failure class this issue exists to remove. So a token that is not `:`-prefixed is an assertion
failure naming it, not a skip. Today's `T` is uniformly shorthand (E2), and the shorthand is what
makes the six cross-project targets work at all. If a scoped entry is ever genuinely wanted, the
gate reds and the author extends the parser deliberately — loud beats silent.

## 4. Components

### `ci/affected-graph/ci_targets.py`

Four inputs, resolved from the repo root the way `run.sh` does:

| # | Source | Used for |
|---|---|---|
| 1 | `moon query tasks --project repo` | `{name → options.runInCI}` for the `repo` project |
| 2 | `moon query tasks` | the set of every task name in the graph |
| 3 | `.github/workflows/ci.yml` | the sole `T=(…)` line |
| 4 | `CLAUDE.md` | the sole full-graph backtick span (D7) |

Three assertions:

- **C1 — forward (the AC).** Every `repo` task with `runInCI ≠ false` appears in `T` as `:name`.
  Failure names each missing task and points at `ci.yml`'s `T`.
- **C2 — reverse (dead target).** Every `:name` in `T` resolves to ≥1 task name in the graph.
  Failure names each dead entry. Generic entries pass because they resolve elsewhere.
- **C3 — docs mirror.** CLAUDE.md's documented target list equals `T` token-for-token, in order,
  after unwrapping line breaks. Failure reports the first divergence by position and prints both
  lists.

Parsing contracts, stated so a future editor knows what may safely change:

- `T` is located by `^\s*T=\((.*?)\)\s*$` over `ci.yml` — one line, parenthesised, no continuation.
  Splitting the array across lines would break the parser, and the rc-2 "not exactly one match"
  control is what makes that break loud rather than silent.
- The CLAUDE.md span is located per D7 and normalised with `" ".join(span.split())` before its
  `:`-prefixed tokens are taken in order.

### `ci/affected-graph/run.sh`

- New `assert_ci_targets()` shelling out to `ci_targets.py`, folding rc 0/1/2 exactly as
  `assert_cargo_moon_parity` does; called from `run_suite`.
- The `--negative-control` branch gains `python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1`,
  alongside the existing `cargo_moon_parity.py --self-test`.

### `moon.yml`

`repo:affected-smoke` gains `CLAUDE.md` to its `inputs` (D9). No other change: the task already
runs `--negative-control` then the real suite, so both halves of this gate execute in CI, and
`ci/affected-graph/**/*`, `.github/workflows/ci.yml` and `moon.yml` are already inputs.

### Docs

- `ci/affected-graph/README.md` — a bullet describing the three checks and their maintenance rule.
- `CLAUDE.md` — a line in the gotcha list noting that a new `repo:*` gate now reds
  `:affected-smoke` until it is in **both** `T` and the documented command, mirroring the existing
  "a new Rust crate reds `:affected-smoke`" note.

## 5. Testing

`ci_targets.py --self-test` drives the three check functions against **in-memory fixtures**, not
the real repo, so the control's verdict does not depend on the tree happening to be aligned. The
fixture table asserts:

| Fixture | Expected |
|---|---|
| a `repo` task absent from `T` | C1 **red**, naming it |
| `install-hooks` (`runInCI: false`) absent from `T` | **green** — AC #3, asserted not assumed |
| a dead `:ghost` entry in `T` | C2 **red**, naming it |
| a project-scoped `repo:promtool` entry in `T` | **red**, naming it (D10) — never silently ignored |
| a generic `:build` resolving in another project | **green** — not flagged by C2 |
| doc missing a target | C3 **red** |
| doc carrying the right targets in the wrong order | C3 **red** (D6) |
| everything aligned | **green** — catches a permanently-red harness |

The last row matters as much as the red ones: a control that only ever proves "it can fail" would
not notice a check wired to fail unconditionally.

Beyond the fixture table, verification is by mutation against the real tree, run and recorded
rather than assumed: add a throwaway `repo:` task to `moon.yml` and confirm the gate names it;
mistype one entry in `T` and confirm both C1 and C2 fire; delete one target from CLAUDE.md's
command and confirm C3 fires; then confirm the unmutated tree is green.

## 6. Limitations

- **L1 — `T` must stay a single-line bash array.** Reformatting it across lines fails the gate as
  an infrastructure error (rc 2). Loud, but it is a constraint on `ci.yml`'s formatting.
- **L1b — a project-scoped entry in `T` is rejected, not supported** (D10). If one is ever wanted,
  the gate reds until the parser is extended deliberately.
- **L2 — the gate asserts membership, not execution.** A target present in `T` and resolvable can
  still do nothing useful — e.g. a task whose `inputs` never match, which is the SMA-534 failure
  class, covered by different assertions in this same script.
- **L3 — other workflows are out of scope.** `security-scan.yml` runs `osv-scanner` directly rather
  than through Moon; nothing here asserts anything about targets outside `ci.yml`'s `T`.
- **L4 — `CLAUDE.md` is the only doc checked.** `CONTRIBUTING.md` and the various READMEs could
  grow their own copy of the command and would not be caught.
- **L5 — a second full-graph example in CLAUDE.md hard-fails the gate** (D7) until someone
  disambiguates. Deliberate, and the failure message says so.

## 7. Acceptance criteria

| Issue AC | Covered by |
|---|---|
| Adding a `repo:*` task to `moon.yml` without adding it to `T` fails the gate, naming the task | C1 |
| The check carries its own control proving it can fail, verified rather than assumed | `--self-test` fixture table, run by `repo:affected-smoke`'s existing `--negative-control` pass; plus the mutation verification in §5 |
| `install-hooks` (and anything else with `runInCI: false`) does not trip it | D3 + its dedicated fixture row |
| CLAUDE.md's documented full-graph command is kept honest by the same check | C3, made reachable by D9 |
