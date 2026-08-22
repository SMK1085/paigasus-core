# SMA-530 — run the `--negative-control` self-test of `release-parity` in CI

**Status:** design approved 2026-08-22
**Issue:** [SMA-530](https://linear.app/smaschek/issue/SMA-530/ci-run-the-negative-control-self-test-of-release-parity-in-ci)
**Related:** SMA-376 (publish-metadata control), SMA-534 (affected-smoke control),
SMA-528 (the live vacuity a control was best placed to catch), SMA-542 / SMA-553
(the guard-the-guard machinery this reuses), SMA-398 / SMA-405 / SMA-406 (the harness itself)

## Problem

`ci/release-parity/run.sh` ships a `--negative-control` mode: it drives `check_case`
with a deliberately wrong expectation (`fix!` in 0.x expected as `0.1.1`, when the
canonical contract says `0.2.0`) and asserts the harness reports red. Nothing runs it
automatically. All three Moon tasks invoke the script bare:

```yaml
  release-parity:     script: 'ci/release-parity/run.sh'
  release-parity-py:  script: 'ci/release-parity/run.sh --ecosystem python-semantic-release'
  release-parity-ts:  script: 'ci/release-parity/run.sh --ecosystem semantic-release'
```

Its two sibling gates both run their control first, under an explicit `set -euo pipefail`
(`repo:publish-metadata`, SMA-376; `repo:affected-smoke`, SMA-534). `release-parity` is
the last uncovered one.

A self-test that runs only when a human remembers is not a control. What it uniquely
catches is a refactor that makes `check_case`'s comparison unable to report red at all —
the real run stays green precisely then, which is why the real run cannot substitute for
it. SMA-528 was a live instance of that class on a sibling gate.

## Measurements

The issue asked for the shape to be chosen from a measurement, not by analogy, on the
theory that `release-parity` builds real git fixtures and its control might be materially
slower. Measured locally (macOS, warm caches, `env -u AI_AGENT -u CLAUDECODE`):

| ecosystem | `--negative-control` | real run | control rc |
| -- | -- | -- | -- |
| `release-plz` | **1s** | 5s | 0 |
| `python-semantic-release` | **3s** | 3s | 0 |
| `semantic-release` | **1s** | 6s | 0 |

The premise does not hold: ~5s total across all three. The control runs one case; the
real run runs the five in `cases.tsv`.

> Reproduction note, local only: `proto bin` emits NDJSON when it detects an agent shell
> (`AI_AGENT` / `CLAUDECODE` set), which breaks `RELEASE_PLZ_BIN` resolution in
> `ecosystems/release-plz.sh:16` and yields rc 2 ("INCONCLUSIVE: infrastructure error").
> Clear those two vars when running the gate by hand. This is an artifact of the agent
> environment, not a repo defect, and does not arise in CI.

## Decisions

### D1 — prepend to the existing tasks; do not add a fourth task

The measurement settles it. A separate `repo:release-parity-selftest` task would cost a
new entry in `ci.yml`'s `T=(…)` array, a matching edit inside CLAUDE.md's
`<!-- ci-targets:begin/end -->` block, and an `:affected-smoke` re-baseline — for no
coverage a prepend does not already buy. Prepending also keeps the control scheduled by
exactly the same affectedness rule as the gate it guards.

### D2 — each ecosystem gets its own control run; one does not cover three

The three tasks share `run.sh`, but `run.sh` is only the tool-agnostic core. Everything
the control actually exercises is per-ecosystem, and the three implementations are
unrelated:

| | `release-plz.sh` | `python-semantic-release.sh` | `semantic-release.sh` |
| -- | -- | -- | -- |
| driver | `release-plz` binary + cargo | `uv run semantic-release` | Node JS API (`semantic-release-next-version.mjs`) |
| fixture | one git repo, two crates | **two separate git repos** (PSR has no path attribution) | one repo, two package dirs, plus a local bare `origin.git` |
| config derived from | `rs/release-plz.toml` | both `py/packages/*/pyproject.toml` | both `ts/packages/*/.releaserc.json` |
| env handling | offline cargo | — | strips `GITHUB_*` CI markers |

The control's job is to prove `check_case` can return 1 **against that plumbing**. A
control on `release-plz` says nothing about whether the semantic-release adapter's
`ecosystem::version` still reads a real version. At 1s/3s/1s, running all three is the
obvious call.

### D3 — control first, under an explicit `set -euo pipefail`

Moon does not enable errexit for `script:` blocks, so a block's exit status is its **last**
command's. Without the pipefail line a failing control is masked by the passing real run,
which makes the change worse than useless — the same trap `repo:promtool`,
`repo:nats-permissions`, `repo:publish-metadata` and `repo:input-liveness` each document.

`run.sh` setting `set -euo pipefail` internally does not help: that governs the script's
own body, not the Moon block that invokes it twice.

Invocation form stays the current bare `ci/release-parity/run.sh` (the file is `+x`,
and this matches `repo:affected-smoke`; `repo:publish-metadata`'s `bash …` prefix is the
minority form).

### D4 — pin the three new invocations in `ci_targets.py`, and decouple the two dicts

The new lines would otherwise be unguarded: deleting `--negative-control` from `moon.yml`
reds nothing, which is exactly the "guard-the-guard" gap SMA-542 spent a PR closing on
`repo:actionlint`. `ci/affected-graph/ci_targets.py` already has the machinery —
`SELF_SCHEDULED_GATES`, checked by `check_self_invocation` from inside the independently
scheduled `repo:affected-smoke`. Today it pins one gate, `input-liveness`.

Add three entries, each pinning all three lines as whole stripped lines:

```python
SELF_SCHEDULED_GATES = {
    "input-liveness": (...),                         # existing
    "release-parity": (
        "set -euo pipefail",
        "ci/release-parity/run.sh --negative-control",
        "ci/release-parity/run.sh",
    ),
    "release-parity-py": (...),                      # --ecosystem python-semantic-release
    "release-parity-ts": (...),                      # --ecosystem semantic-release
}
```

`set -euo pipefail` is pinned as a first-class required line, not decoration: per D3,
deleting it touches neither invocation's text, so a pin that covered only the two commands
would stay green while a failing control is silently swallowed.

**Whole-line matching is load-bearing here in both directions.** `ci/release-parity/run.sh`
is a strict prefix of `ci/release-parity/run.sh --negative-control`, so a substring test
would let the REAL run be deleted while the pin stayed green. Conversely, for the `-py` and
`-ts` tasks the real-run line (`… run.sh --ecosystem semantic-release`) is a strict prefix
of the control line (`… run.sh --ecosystem semantic-release --negative-control`), so the
same hazard runs the other way. `SELF_SCHEDULED_GATES` already strips both sides (Moon task
scripts are indented inside YAML) and compares against a set of whole lines, which is
correct for all six strings.

**The coupling must be relaxed.** `ci_targets.py:1295` currently asserts
`set(SELF_SCHEDULED_GATES) == set(SELF_TASK_EXPECTED_GLOBS)`, and the latter pins a gate's
`inputs`. That equality forces every script-pinned gate to also have its input globs
duplicated into `ci_targets.py`. For `input-liveness` that duplication is the point — its
`inputs: ['**/*']` is load-bearing, and narrowing it would switch off exactly what the gate
exists to notice. For `release-parity*` it is not: their globs are an ordinary
affectedness question, already asserted live and generically by `repo:input-liveness`, and
copying three narrow 4–7-entry lists here would create a second maintenance site that reds
on every legitimate `inputs` edit while buying no safety.

Relax it to a subset test — the globs pin becomes opt-in, the script pin the superset:

```python
if not set(SELF_TASK_EXPECTED_GLOBS) <= set(SELF_SCHEDULED_GATES):
    raise ...
```

The direction is deliberate and still catches the failure that matters: a
`SELF_TASK_EXPECTED_GLOBS` entry whose gate is no longer script-pinned is an orphan
asserting inputs for a gate nothing else checks, and stays an error. A script-pinned gate
with no globs entry is the normal case.

### D5 — reachability is already satisfied; do not widen the tasks' own inputs

`check_self_invocation` runs only when `repo:affected-smoke` is scheduled. That gate
already lists `moon.yml` among its inputs, so a PR deleting a pinned line re-keys it and
the pin fires. No new input is needed — unlike the SMA-542 case, which had to add
`ci/actionlint/**/*` for the same reason.

Measured, on a `moon.yml`-only edit (`moon query tasks --affected`):

```
repo:actionlint
repo:affected-smoke
repo:input-liveness
```

The three `release-parity*` tasks are **not** selected: their own `script:` lives in
`moon.yml`, but `moon.yml` is not among their inputs. Two consequences.

First, we deliberately do **not** add `moon.yml` to their inputs. It would run ~14s of
real git-fixture work on every gate PR (gate PRs edit `moon.yml` constantly), and it is
not what protects the lines — the `ci_targets.py` pin is, and it is reachable.

Second, and this is the part that needs handling rather than noting: without it the new
control would ship on a PR that never executed it. Hence D6.

### D6 — document the contract in `ci/release-parity/README.md`, which also re-keys the tasks

`ci/release-parity/**/*` is an input to all three tasks, so editing the README selects
them and this PR exercises its own change in CI rather than shipping unverified. The
documentation is worth writing on its own merits; the re-keying is a genuine second
reason, and both are recorded in the moon.yml comment so a future reader does not "tidy"
the README edit out of a similar PR.

## Non-goals

- **Closing the same gap for `publish-metadata`, `affected-smoke` and
  `error-code-single-site`.** All three run a self-test from `moon.yml` today and none is
  script-pinned. That is a real, identical exposure, but it is not SMA-530's scope, and
  each needs its own verification pass. D4's dict-decoupling is what makes adding them a
  one-line change later; file a follow-up.
- **Strengthening the control itself.** It exercises one case (`neg-fix-bang`) with a
  literal expectation, bypassing `resolve_expected` / `ecosystem::expected`. That is not a
  hole: for `semantic-release`, deleting `ecosystem::expected` makes the *real* run red
  (it would expect `0.2.0` and get `1.0.0`), so the divergence resolver is already
  covered from the other side.
- Adding a fourth ecosystem, or touching `cases.tsv`.

## Verification

Per the issue: neuter a rule, confirm the Moon task exits non-zero **and** that the real
run does not execute afterwards, then restore.

1. **The control bites, per ecosystem.** Stub `check_case` to `return 0` unconditionally.
   For each of the three tasks: `moon run repo:<task> --force` exits non-zero, and
   `== all parity cases passed ==` never appears in the output — proving the real run was
   not reached. Restore.
2. **The real run still passes unmodified.** All three tasks green on a clean tree
   (guards against a control that reds legitimately-correct code).
3. **`set -euo pipefail` is load-bearing.** With it deleted and the control forced to
   fail, confirm the task reports success — the failure mode D3 describes — then restore.
   (Demonstration, not a committed test.)
4. **The pin bites.** Delete each pinned line in turn from `moon.yml`;
   `ci/affected-graph/run.sh` reports the missing call site and exits non-zero. Also
   confirm `moon.yml`-only edits still select `repo:affected-smoke` (D5).
5. **The coupling relaxation bites in the right direction.** An entry in
   `SELF_TASK_EXPECTED_GLOBS` with no `SELF_SCHEDULED_GATES` counterpart still raises;
   the reverse does not.
6. **`ci_targets.py --self-test` and `ci/affected-graph/run.sh --negative-control` pass**
   with the new entries; update the self-test fixtures if the new keys break them.
7. **Full graph** as CI runs it, per CLAUDE.md's marker-delimited command.

## Files touched

| file | change |
| -- | -- |
| `moon.yml` | three `script:` blocks (+ comments recording D1–D3, D5, D6) |
| `ci/affected-graph/ci_targets.py` | three `SELF_SCHEDULED_GATES` entries; equality → subset at the pairing assert; comments for D4 |
| `ci/release-parity/README.md` | the control-runs-in-CI contract (D2, D3, D6) |
