# SMA-539 — Linting the CI tooling: inline workflow bash and `ci/**/*.py`

**Status:** design (rev 1)
**Issue:** SMA-539 (absorbs SMA-555)
**Branch:** `feature/sma-539-repo-lint-the-ci-tooling-inline-workflow-bash-and-cipy`
**Verified against `main` @ `67cbf5e` (moon 2.5.3, proto 0.61.1, uv 0.11.16, actionlint 1.7.12, ruff 0.16.5).**

Two surfaces in this repo carry a convention that nothing enforces. `.github/workflows/**` holds
648 lines of inline bash that no linter reads, because SMA-525 deliberately switched actionlint's
shellcheck integration off. `ci/**/*.py` is expected to satisfy `py/pyproject.toml`'s Ruff rule
set, but `.moon/tasks/python.yml` scopes `ruff check` to the `py` project and nothing else looks
at `ci/`.

**The central finding of this spec is that Half A is far cheaper than the issue assumed, and Half
B is far more expensive.** The issue expected Half A to need a new gate, a new vendored tool and
possibly a 120-line bash extraction, and expected Half B to land with no cleanup. Measured: Half A
needs no new gate at all and its corpus is already clean, while Half B's corpus carries 27
violations. §1 records every number; §2 and §3 turn them into decisions.

---

## 1. Measured baseline

Every number below was measured on this branch at `67cbf5e`, not reasoned about. Two of the
issue's own stated baselines were stale and are corrected here.

### 1.1 Inline bash volume — 5.4x the issue's estimate

The issue says "roughly 120 lines" counting `ci.yml` and `prebuild.yml`. Parsing every workflow's
`jobs.*.steps[].run` with PyYAML:

| Workflow | `run:` blocks | multi-line | bash lines |
|---|---:|---:|---:|
| `wheels.yml` | 22 | 11 | 273 |
| `release.yml` | 27 | 16 | 207 |
| `prebuild.yml` | 18 | 9 | 107 |
| `ci.yml` | 14 | 11 | 52 |
| `images.yml` | 2 | 1 | 6 |
| `security-scan.yml` | 3 | 0 | 3 |
| **total** | **86** | **48** | **648** |

The estimate was not wrong when written; `wheels.yml` (SMA-578) and `release.yml` (SMA-579,
SMA-603) landed after it. This matters because it is the number the extraction option in §2.3 has
to move.

### 1.2 The bash corpus is already clean

`actionlint -shellcheck=<path> -pyflakes= .github/workflows/*.yml` over all six workflows:
**0 findings, exit 0.**

A green from a linter that might not be running is worth nothing, so this was mutation-checked
before being believed. A fixture containing `rm -rf $f` inside a `for f in $FILES` loop:

* with the gate's current `-shellcheck=` — **exit 0, no output**
* with `-shellcheck=<path>` — `SC2086 ... Double quote to prevent globbing and word splitting`, **exit 1**

So shellcheck genuinely fires, the current configuration genuinely suppresses it, and the real
corpus genuinely has nothing to report. Half A ships no cleanup wave.

### 1.3 What actionlint asks shellcheck to check

Read out of the actionlint 1.7.12 binary: it invokes shellcheck with `--norc`, prepends `set -e`
to each script, and passes the exclusion list

```
SC1091,SC2194,SC2050,SC2153,SC2154,SC2157,SC2043
```

Probed empirically, one construct per fixture:

| construct | reported |
|---|---|
| `rm -rf $UNQUOTED` | `SC2086` |
| `echo $(ls \| grep x)` | `SC2005 SC2010 SC2046` |
| `x=1; echo "$y"` | `SC2034` |
| `if [ $a == $b ]; then …` | `SC2086` |
| `cd /tmp` | *(none)* |

Two suppressions are structural rather than configured, and both are recorded as limitations in
§8: `SC2148` (missing shebang) cannot fire because actionlint supplies the shell itself, and
`SC2164` (`cd` without `|| exit`) cannot fire because the injected `set -e` already aborts on a
failed `cd`. What remains is the quoting and word-splitting class, which is the class that
actually bites in CI.

### 1.4 Cost of enabling it

| | wall clock |
|---|---:|
| `actionlint -shellcheck= -pyflakes=` (today) | 0.106s |
| `actionlint -shellcheck=<path> -pyflakes=` | 0.359s |

`+0.25s` against a gate `moon.yml` documents at 15-20s warm.

### 1.5 `ci/**/*.py` — 27 violations, not zero

The issue measured two files on 2026-08-19 and found them clean. The corpus is now ten tracked
files and was never linted. Against `py/pyproject.toml` (`select = ["E","F","W","I","N","UP","B","A","C4","SIM","TCH","RUF"]`,
`line-length = 200`, `ignore = ["E501"]`), ruff 0.16.5:

| rule | count | files |
|---|---:|---|
| `RUF100` unused-noqa | 6 | `publish-metadata/categories.py`, `release-plan/release_plan.py`, `workflow-credentials/workflow_credentials.py` |
| `E402` import-not-at-top | 5 | `workflow-credentials/workflow_credentials.py` |
| `RUF005` literal-concatenation | 3 | `publish-metadata/categories.py` |
| `UP017` `datetime.UTC` | 3 | `publish-metadata/categories.py` |
| `N818` error-suffix-on-exception | 3 | `pyo3-stub/check.py`, `release-plan/release_plan.py`, `workflow-credentials/workflow_credentials.py` |
| `UP031` printf-formatting | 2 | `affected-graph/cargo_moon_parity.py` |
| `UP014` NamedTuple class syntax | 1 | `pyo3-stub/check.py` |
| `N806` non-lowercase-in-function | 1 | `pyo3-stub/check.py` |
| `B904` raise-without-from | 1 | `actionlint/release_guard.py` |
| `C420` `dict.fromkeys` | 1 | `actionlint/release_guard.py` |
| `SIM300` yoda-condition | 1 | `affected-graph/cargo_moon_parity.py` |
| **total** | **27** | 6 of 10 files |

12 are ruff-autofixable. Four of the ten tracked files are already clean.

`ruff check --config py/pyproject.toml ci/` — passing the directory rather than an explicit file
list — reports the **same 27**. Ruff honours `.gitignore`, so it skips the untracked
`ci/release-plan/.venv` and `ci/workflow-credentials/.venv` trees on its own. No exclusion needed.

### 1.6 `ruff format` would rewrite a third of the corpus

| file | changed lines | total |
|---|---:|---:|
| `actionlint/release_guard.py` | 1612 | 2999 |
| `affected-graph/cargo_moon_parity.py` | 1457 | 4400 |
| `affected-graph/ci_targets.py` | 907 | 2727 |
| `pyo3-stub/check.py` | 278 | 1522 |
| `affected-graph/task_inputs.py` | 224 | 713 |
| `workflow-credentials/workflow_credentials.py` | 187 | 706 |
| `publish-metadata/categories.py` | 125 | 797 |
| `release-plan/release_plan.py` | 115 | 532 |
| `http-extractor/check.py` | 87 | 734 |
| `error-registry/check.py` | 79 | 469 |
| **total** | **5071** | **~15600** |

All ten files would be reformatted. §5.2 decides against gating it, and records why.

### 1.7 Reachability premises

Two facts the design leans on, both checked rather than assumed:

* **`repo:actionlint` runs on every CI run.** Its `inputs` are `['**/*']` (`moon.yml`, with a
  comment explaining why the narrow-inputs convention is deliberately not followed there).
* **It already shells out to `uv run --locked --project py`.** `ci/actionlint/run.sh:4518`,
  the `release_guard_py` wrapper, called unconditionally on the full-gate path at `:4525`
  (`--fixture-count`, under `|| infra`) and `:5433`. It also runs
  `uv run --locked --project ci/release-plan` at `:4554`.

Together these say the `py` environment is materialised on every CI run regardless of what this
issue does. §4.1 is entirely downstream of that.

---

## 2. Half A — sourcing shellcheck

### 2.1 The blocker, re-measured

SMA-525's D2 rejected a proto plugin because shellcheck's release ships no checksums asset. Still
true on 2026-09-02: `v0.11.0` (published 2025-08-04) carries **13 archives and no checksums
asset**. A `.proto/plugins/shellcheck.toml` would be the first vendored tool in this repo
downloaded without integrity verification.

### 2.2 Decision: `shellcheck-py` from PyPI, resolved through uv

`shellcheck-py==0.11.0.1` wraps the official `shellcheck` 0.11.0 binary, and **every** path to it
is checksummed:

* **Wheel path** — prebuilt wheels for `macosx_11_0_arm64`, `macosx_10_9_x86_64`,
  `manylinux_2_17_x86_64` and `win_amd64`, binary bundled. `uv.lock` records a sha256 per wheel.
* **sdist path** (linux aarch64/riscv64/armv6hf, which have no wheel) — the sdist's `setup.cfg`
  drives `setuptools-download`, which carries a **hand-pinned sha256 per asset**, including
  aarch64. `uv.lock` records a sha256 for the sdist itself.

The republisher's pins were verified against koalaman's own release assets rather than trusted:

| asset | verdict |
|---|---|
| `shellcheck-v0.11.0.darwin.aarch64.tar.xz` | **MATCH** |
| `shellcheck-v0.11.0.linux.x86_64.tar.xz` | **MATCH** |
| `shellcheck-v0.11.0.linux.aarch64.tar.xz` | **MATCH** |

The mechanism was then verified end to end in a throwaway uv project: `uv lock` produced a lock
carrying the sdist hash and all four wheel hashes, and

```
uv run --locked --project <dir> python3 -c 'import shutil; print(shutil.which("shellcheck"))'
```

printed `<dir>/.venv/bin/shellcheck` at rc 0, and that binary reports `version: 0.11.0`.

**The honest cost** is one supply-chain hop: `shellcheck-py` is a third-party republisher (the
canonical pre-commit mirror), not koalaman. The trade is "first-party, unverifiable" against
"one hop, verified at three digests and pinned by hash in a committed lockfile". This spec takes
the second, and §8 L1 records the residual.

### 2.3 Rejected: extracting `run:` blocks into `ci/*.sh`

The issue calls this "may be the best of them". It is not, for three measured reasons.

1. It is a **648-line** refactor (§1.1), not the 120 lines the issue costed it at, and the bulk
   of it is `release.yml` and `wheels.yml` — the credentialed publish paths, where a transcription
   error is a live-release incident rather than a red PR.
2. Many blocks interpolate `${{ }}`, which does not survive a move into a script without
   restructuring each one into an argument or an env var — turning a mechanical move into a
   per-block redesign.
3. It buys nothing that §2.2 does not already buy. The corpus is clean either way (§1.2), and an
   extracted script gets exactly the shellcheck that enabling the integration gets.

The composition argument the issue makes for it — that extracted scripts land under `ci/`, which
Half B covers — is real but backwards: it would make Half B's corpus larger and Half A's diff
enormous, to gate bash that §2.2 already gates in place.

### 2.4 Rejected: CI-only install

Reintroduces exactly the host-dependent strictness SMA-525 refused, differing only in that the
divergence is signposted. §2.2 costs no more and has no divergence.

---

## 3. Half A — design

### 3.1 No new gate

actionlint already owns this integration; SMA-525 switched it off. Turning it on is a change to
one existing gate, so **Half A carries none of the five registry obligations**. Only Half B pays
them. This is the single largest simplification against the issue's assumed shape.

### 3.2 Changes to `ci/actionlint/run.sh`

* Add `shellcheck-py==0.11.0.1` to `py/pyproject.toml`'s `[dependency-groups] dev`. No new
  lockfile, no new proto plugin, no new `.venv` — the file already resolves through
  `uv run --locked --project py` (§1.7).
* Resolve the binary through that same wrapper shape and assert `[ -x ]` on the result, the idiom
  `ci/release-parity/ecosystems/release-plz.sh` established. `--locked` is mandatory for the same
  reason `release_guard_py` carries it: a bare `uv run --project py` re-locks `py/uv.lock` as a
  side effect.
* Replace `ARGS=(-shellcheck= -pyflakes=)` at `:79` with the resolved path. **`-pyflakes=` stays
  off**: no `run:` block in this repo executes inline Python, so it would gate an empty corpus
  while raising the same sourcing question a second time.

### 3.3 Fail closed

If shellcheck cannot be resolved, `run.sh` aborts `infra` (rc 2). It must **never** fall back to
`-shellcheck=`.

This is the whole point. A silent downgrade to "strictness is a property of the host" is precisely
the failure SMA-525 refused, and a gate that skips its own sub-check on a green is worse than one
that never had it. rc 2 is already distinct from rc 1 throughout this gate, and CLAUDE.md's
`repo:affected-smoke` entry records the precedent: an infrastructure abort is a distinct verdict
from a red, and never a false green.

### 3.4 `shellcheck_self_test` — the 14th

`ci/actionlint/run.sh:39-40` holds `SELF_TESTS_RAN=0` / `SELF_TEST_COUNT=13`. Add
`shellcheck_self_test`, call it from `run_self_tests`, and bump the count to **14**, extending the
inline comment list at `:40`.

That one constant is asserted **three** ways, so a half-done addition reds rather than passing
quietly: `assert_self_tests_ran` (`:4582`, called at `:4609`) compares the runtime
`SELF_TESTS_RAN`; a definitions count at `:4625` fails if the number of `*_self_test` functions
defined disagrees ("a fixture table that is not called from `run_self_tests` guards nothing"); and
a call-site count at `:4665` fails if the number of invocations disagrees. `selftest_mutation_battery`
picks the new table up automatically.

The fixture asserts that a script actionlint accepts but shellcheck rejects produces a **red**.
Without it, a future regression to "shellcheck not found, integration skipped" leaves the gate
green over a corpus it never inspected — which is the SMA-525 failure re-created inside the fix
for it. This is the AC 9 mutation proof made permanent rather than performed once.

Per CLAUDE.md, adding a `*_self_test` also touches `ci_targets.py`'s `ACTIONLINT_SH_CALL_SITES`
accounting: the gate asserts invocations **and** definitions.

---

## 4. Half B — `repo:ruff-ci`

### 4.1 Decision: route through `--project py`, not a dedicated project

The obvious precedent is `ci/workflow-credentials/`, whose `pyproject.toml` comment argues
explicitly against `--project py`: `py/` is a `[tool.uv.workspace]` root whose member
`paigasus-kernel` depends on `paigasus-py-bindings` by path, and that crate builds with maturin —
so `uv run --project py` compiles a PyO3 cdylib. SMA-593 measured 0.073s warm for a dedicated
one-dependency project and declined to put a cdylib build on every workflow-edit PR.

**That premise does not hold for this gate.** `repo:actionlint` has `inputs: ['**/*']` and calls
`release_guard_py` unconditionally (§1.7), so the `py` environment is already synced on every CI
run — including on a PR that touches only `ci/**/*.py`. The cdylib cost is paid before
`repo:ruff-ci` is reached, so routing through `py` adds **zero** marginal cost.

Warm, for the record: `uv run --project py --locked ruff check … ci/` is 0.235s;
`uv run --project ci/workflow-credentials` is 0.064s. Both are noise.

What `--project py` buys that a dedicated project does not:

* **No second lockfile**, so no second ruff version, so no drift between what `py:lint` runs and
  what `repo:ruff-ci` runs. A dedicated project would have needed a version-lockstep assertion
  between two `uv.lock` files purely to hold that invariant — a `repo:version-lockstep`-shaped
  mechanism invented to solve a problem this decision does not create.
* **AC 5 satisfied twice over** — one rule set *and* one tool version, from one file.

### 4.2 The check

```
uv run --locked --project py ruff check --config py/pyproject.toml ci/
```

Directory rather than file list, which is safe because ruff honours `.gitignore` and reports the
identical 27 (§1.5). `--config py/pyproject.toml` is what makes AC 5 true: one rule set, no copy.

### 4.3 Corpus-liveness assertion

A gate that walks a directory can be silently emptied by moving that directory. This is the
SMA-553 failure class, and `repo:input-liveness` cannot reach it: CLAUDE.md records that
`task_inputs.py`'s `_repo_tasks` is keyed to `projects.get("repo")` by exact project id, so it
liveness-checks a `repo:*` task's **declared inputs** and never proves that a scan found anything.

So `ci/ruff/run.sh` asserts that the set of files ruff actually inspected is **non-empty** and
**equals** the tracked `ci/**/*.py` set, comparing against `git ls-files`. Wiring `ruff check` to
a corpus it cannot prove it read is the one way this gate lies while staying green.

### 4.4 Shape

`ci/ruff/run.sh` follows `repo:workflow-credentials` exactly — `--self-test`, `--negative-control`,
then the real run, all four lines under an explicit `set -euo pipefail` in one `script:` block,
because Moon does not enable errexit for `script:` blocks and takes the block's status from its
last command.

```yaml
  ruff-ci:
    description: 'Lint ci/**/*.py against py/pyproject.toml''s Ruff rule set (SMA-539).'
    script: |
      set -euo pipefail
      bash ci/ruff/run.sh --self-test
      bash ci/ruff/run.sh --negative-control
      bash ci/ruff/run.sh
    toolchain: 'system'
    inputs:
      - 'ci/**/*.py'
      - 'ci/ruff/**/*'
      - 'py/pyproject.toml'
      - 'py/uv.lock'
      - '.prototools'
```

`py/pyproject.toml` and `py/uv.lock` are load-bearing, not padding: the first is the rule set, the
second pins the ruff version. Without them a rule change or a ruff bump would leave a cached PASS
standing. `.prototools` pins `uv` itself, exactly as the `release-parity*` tasks list it.

---

## 5. Baseline cleanup

### 5.1 Fix all 27, no carve-outs

No `per-file-ignores`, no rule exclusions. A carve-out on day one weakens the gate and diverges
from `py/`'s rule set, which AC 5 forbids. Blast radius was checked before committing to this:

* **`N818` × 3** — `Refused`, `Inconclusive`, `AssertionFailure`. Each appears in its own module,
  its gate's `README.md`, and its gate's `run.sh` — but in `run.sh` **only inside comments**
  (`ci/release-plan/run.sh:156`, `ci/workflow-credentials/run.sh:65`). No whole-line pin array in
  `ci_targets.py` or `ci/actionlint/run.sh` contains any of them. The renames are mechanical.
* **`E402` × 5** — `workflow_credentials.py` puts `glob`/`os`/`re`/`tempfile`/`yaml` after the RC
  constants and `class InfraError`. There is no runtime reason: `import yaml` is not guarded by a
  `try`, and nothing between the import groups depends on ordering. The layout was transcribed
  from the SMA-593 plan's own sketch. Reordering is safe.
* **`RUF100` × 6** — `# noqa: BLE001 — <prose>` where `BLE001` is not in `py/`'s select list.
  Ruff's autofix deletes the **whole comment**, prose included. Fix by hand: drop the `noqa:`
  token, keep the explanation as a plain comment. Six lines of "why a broad `except` is correct
  here" are worth more than the two characters saved.
* The remaining 13 (`RUF005`, `UP017`, `UP031`, `UP014`, `N806`, `B904`, `C420`, `SIM300`) are
  mechanical, and `RUF005` is the exact rule SMA-541 already fixed by hand in this corpus.

### 5.2 `ruff format` is deliberately not gated (AC 7)

**Decision: `check` only.** Recorded here, in `ci/ruff/README.md`, and in CLAUDE.md, so the
question is closed rather than ambiguous.

Two reasons, in order of weight:

1. **A pin-corruption hazard.** Several of these files build whole-line pin arrays
   (`SELF_SCHEDULED_GATES`, `RUN_SH_CALL_SITES`, `ACTIONLINT_SH_CALL_SITES`,
   `T_CARGO_LOCK_SH_CALL_SITES`) out of implicitly-concatenated string literals. `ruff format`
   joining such a concatenation changes the pinned literal's **value** — a silent pin break whose
   symptom is a *different* gate reding, on a PR that appears to be pure formatting.
2. **5071 changed lines across ~15600** (§1.6), driven by `line-length = 200` joining hand-wrapped
   lines and collapsing hand-aligned fixture tables. These files are roughly 60% comment by
   design; the wrapping is a readability decision, not an accident.

This is a decision not to gate formatting, not a claim that the corpus is well formatted. Anyone
wanting the reformat should take it as its own issue with its own pin-safety audit.

---

## 6. Proof each gate can red (AC 9)

Four mutations, each performed and its output recorded in the plan — not reasoned about.

| # | mutation | expected |
|---|---|---|
| M1 | plant an `SC2086` in a real workflow's `run:` | `repo:actionlint` **rc 1** |
| M2 | make shellcheck unresolvable (drop the dep / break the path) | `repo:actionlint` **rc 2**, never a green |
| M3 | plant a `RUF005` in a `ci/**/*.py` file | `repo:ruff-ci` **rc 1** |
| M4 | remove any one of the five registrations | `repo:affected-smoke` **rc 1** |

M2 is the one that matters most and the one a "does the gate work?" check would skip: it proves
the §3.3 fail-closed property, which is the entire difference between this design and the
opportunistic integration SMA-525 rejected.

`--self-test` and `--negative-control` make M1/M3 permanent rather than one-time.

---

## 7. Registry wiring (AC 8)

Half A adds nothing here (§3.1). `repo:ruff-ci` pays all five:

1. **`ci.yml`'s `T=(…)`** — append `:ruff-ci`, keeping it a single-line bash array (SMA-541).
2. **CLAUDE.md's marker-delimited command** — the same target, between the
   `<!-- ci-targets:begin -->` / `<!-- ci-targets:end -->` markers. `ci_targets.py` asserts the two
   agree, and that every `T` entry resolves to a CI-eligible task — which matters because
   `moon ci` exits **0** on a target resolving to nothing.
3. **`SELF_SCHEDULED_GATES`** — its four `moon.yml` lines (`set -euo pipefail`, `--self-test`,
   `--negative-control`, the real run), matched as whole lines after stripping.
4. **`SELF_TASK_EXPECTED_GLOBS`** — all five literal `inputs` from §4.4. Not
   `SELF_TASK_GLOBS_EXEMPT`; holding both is itself reported.
5. **`T_AFFECTED_SMOKE_REQUIRED_INPUTS`** in `ci/actionlint/run.sh` — required **only if**
   `ci/ruff/run.sh` script-pins anything. The §4.3 liveness assertion pins no line in another
   file, so as designed this obligation is **not** triggered. If the implementation adds a script
   pin, the floor entry becomes mandatory: without it the pin stays green on exactly the PR that
   breaks it.

Half A's `SELF_TEST_COUNT` 13 → 14 (§3.4) is not one of the five, but it is the same class of
obligation and reds `repo:actionlint` if missed.

---

## 8. Non-goals and limitations

* **L1 — one supply-chain hop.** `shellcheck-py` is a third-party republisher. Three digests were
  verified against koalaman's assets (§2.2) and `uv.lock` pins artifact and hash, but a future
  version bump re-opens the question and should re-verify rather than assume.
* **L2 — `SC2148` and `SC2164` cannot fire.** Structural, not configured (§1.3). In particular a
  `cd` whose failure would matter is covered by actionlint's injected `set -e` rather than by
  shellcheck, so a `run:` block that overrides `set +e` is outside both.
* **L3 — no wheel for linux aarch64/riscv64/armv6hf.** Those platforms take the sdist path, which
  is checksummed but requires a build (`setuptools-download`) and network at install time. CI is
  `ubuntu-latest` x86_64 and the dev box is macOS arm64, so both take the wheel path. An aarch64
  Linux contributor is unmeasured.
* **L4 — `-pyflakes=` stays off.** No `run:` block executes inline Python today. Nothing asserts
  that remains true, so a future workflow embedding Python is unlinted.
* **L5 — formatting is ungated by decision** (§5.2), so `ci/**/*.py` formatting will continue to
  drift. The gate says nothing about it.
* **L6 — `repo:ruff-ci` covers `ci/` only.** There is no tracked Python outside `py/` and `ci/`
  today (measured: zero files), so the two gates partition the corpus — but nothing asserts that
  partition, and Python added at a third location would be unlinted by both.
* **L7 — the liveness assertion proves the corpus was read, not that it is complete.** It compares
  ruff's inspected set to `git ls-files 'ci/**/*.py'`. A Python file added under `ci/` with a
  non-`.py` extension, or one that is gitignored, is invisible to both sides and so agrees
  vacuously.

---

## 9. Acceptance criteria mapping

| AC | where |
|---|---|
| **A1** inline bash linted deterministically, same on dev box and CI | §2.2, §3.2 — one hash-pinned binary from a committed lockfile, no host lookup |
| **A2** integrity story written down where the next person finds it | §2.2 here, plus `ci/actionlint/README.md` and the `py/pyproject.toml` dep comment |
| **A3** no unchecksummed download without explicit justification | §2.2 — every path checksummed; nothing to justify |
| **B4** a Ruff violation under `ci/` fails CI | §4.2, proven by M3 (§6) |
| **B5** same rule set as `py/`, not a second copy | §4.1, §4.2 — `--config py/pyproject.toml`, and one ruff version via `--project py` |
| **B6** every existing `ci/**/*.py` passes on an unmutated tree | §5.1 — all 27 fixed, from the fresh §1.5 baseline |
| **B7** `ruff format` decided explicitly and recorded | §5.2 — check-only, with the 5071-line measurement and the pin hazard |
| **C8** five registry obligations, or `:affected-smoke` reds | §7 |
| **C9** each gate proven to red by mutation | §6 — M1-M4, plus `--self-test`/`--negative-control` making them permanent |
