# SMA-539 — Linting the CI tooling: inline workflow bash and `ci/**/*.py`

**Status:** design (rev 2 — reworked after adversarial challenge; see §10 for the changelog)
**Issue:** SMA-539 (absorbs SMA-555)
**Branch:** `feature/sma-539-repo-lint-the-ci-tooling-inline-workflow-bash-and-cipy`
**Verified against `main` @ `67cbf5e` (moon 2.5.3, proto 0.61.1, uv 0.11.16, actionlint 1.7.12, ruff 0.16.5, shellcheck 0.11.0).**

Two surfaces in this repo carry a convention that nothing enforces. `.github/workflows/**` holds
648 lines of inline bash that no linter reads, because SMA-525 deliberately switched actionlint's
shellcheck integration off. `ci/**/*.py` is expected to satisfy `py/pyproject.toml`'s Ruff rule
set, but `.moon/tasks/python.yml` scopes `ruff check` to the `py` project and nothing else looks
at `ci/`.

**Half A is cheaper than the issue assumed and Half B is more expensive.** The issue expected Half
A to need a new gate, a new vendored tool and possibly a bash extraction, and expected Half B to
land with no cleanup. Measured: Half A needs no new gate, while Half B's corpus carries 27
violations.

**But Half A's coverage is narrower than "lint the inline bash" suggests, and §1.8 is the most
important measurement in this spec:** actionlint replaces `${{ }}` expressions with inert
placeholders before shellcheck sees them, so the entire GitHub-expression interpolation class —
the dominant hazard in `release.yml` and `wheels.yml` — stays invisible. This spec ships the
coverage it can and says plainly what it does not cover, rather than claiming the corpus is clean.

---

## 1. Measured baseline

Every number below was measured on this branch at `67cbf5e`. Two of the issue's own baselines were
stale and are corrected here; two claims from rev 1 of this spec were wrong and are corrected too
(§10).

### 1.1 Inline bash volume — 5.4x the issue's estimate

The issue says "roughly 120 lines" for `ci.yml` plus `prebuild.yml`. Parsing every workflow's
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
SMA-603) landed after it. This is the number §2.3's extraction option would have to move.

### 1.2 The bash corpus reports nothing today

`actionlint -shellcheck=<path> -pyflakes= .github/workflows/*.yml` over all six workflows:
**0 findings, exit 0.**

This is a true statement about what the tool reports, **not** evidence that the bash is sound —
§1.8 gives the reason. Read it before quoting this number.

A green from a linter that might not be running is worth nothing, so it was mutation-checked
first. A fixture containing `rm -rf $f` inside `for f in $(ls)`:

* with the gate's current `-shellcheck=` — **exit 0, no output**
* with `-shellcheck=<path>` — `SC2086`, **exit 1**

So shellcheck fires, and the current configuration suppresses it.

### 1.3 What actionlint asks shellcheck to check

Probed empirically, one construct per fixture (this is the evidence; the `strings`-derived
exclusion list below is corroboration, not proof, since it does not establish that the list is
applied unconditionally):

| construct | reported |
|---|---|
| `rm -rf $TARGET` (unknown var) | `SC2086` |
| `for f in $(ls); do rm -rf $f; done` | `SC2045 SC2086` |
| `echo $(ls \| grep x)` | `SC2005 SC2010 SC2046` |
| `x=1; echo "$y"` | `SC2034` |
| `if [ $a == $b ]; then …` | `SC2086` |
| `VAR=/some/path; rm -rf $VAR` | *(none — shellcheck infers a literal assignment is safe)* |
| `cd /tmp` | *(none)* |

Corroborating, read out of the actionlint 1.7.12 binary: it invokes shellcheck with `--norc`,
prepends `set -e`, and passes `SC1091,SC2194,SC2050,SC2153,SC2154,SC2157,SC2043`.

Two suppressions are structural rather than configured (§9 L2): `SC2148` (missing shebang) cannot
fire because actionlint supplies the shell, and `SC2164` (`cd` without `|| exit`) cannot fire
because the injected `set -e` already aborts on a failed `cd`.

### 1.4 Cost

`actionlint` alone over all six workflows: **0.106s → 0.359s**.

That is the binary, not the gate. The gate also pays one `uv run --locked --project py` to resolve
the binary, and §3.2 places that resolution **after** the `--self-test` early exit specifically so
`--self-test` and check 9's mutant fan-out pay nothing (§3.4). Baseline for the end-to-end
re-measurement the plan must record: `moon run repo:actionlint --force` is **35.1s** on this
branch at `67cbf5e` (hash `5fbecae8`) — itself above the "15-20s warm" its `moon.yml` comment
documents, so the delta must be measured against 35.1s, not against the stale figure.

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

`ruff check --config py/pyproject.toml ci/` — the directory form — reports the same 27, because
ruff's **default `exclude`** already lists `.venv` and `py/pyproject.toml` uses `extend-exclude`,
which preserves defaults. (`respect-gitignore` also happens to cover it, but that is a setting
someone could flip; the default exclude is the load-bearing mechanism.) §4.2 nevertheless passes an
explicit file list, for the reason in §4.3.

### 1.6 `ruff format` would rewrite roughly a fifth of the corpus

`ruff format --diff` over the ten tracked files: **2,998 existing lines removed, 2,073 written**,
across ~15,600 lines total. All ten files would be reformatted.

(Rev 1 reported "5071 changed lines". That figure was `grep -c '^[+-]'`, which counts both sides of
the diff — it double-counts. 2,998 is the number of existing lines rewritten.)

### 1.7 Reachability premises

* **`repo:actionlint` runs on every CI run.** Its `inputs` are `['**/*']` (`moon.yml`).
* **It already shells out to `uv run --locked --project py`** — `ci/actionlint/run.sh:4518`
  (`release_guard_py`), called unconditionally on the full-gate path at `:4525` and `:5433`. It
  also runs `uv run --locked --project ci/release-plan` at `:4554`.
* **`repo:affected-smoke` already declares `ci/**/*`** (`moon.yml:205`) and that glob is floored in
  `T_AFFECTED_SMOKE_REQUIRED_INPUTS` (`ci/actionlint/run.sh:2123`), so a script pin under `ci/` is
  reachable. The three predecessors nonetheless each added a **narrow** sibling glob; §7 follows
  that precedent and explains why.

### 1.8 The `${{ }}` blind spot — the measurement that bounds Half A

actionlint replaces GitHub expressions with inert placeholders before handing the script to
shellcheck. Structurally identical constructs, one shell variable and one expression:

| construct | reported |
|---|---|
| `rm -rf $TARGET` | **`SC2086`** |
| `rm -rf ${{ github.event.inputs.target }}` | **none** |
| `for f in ${{ github.event.inputs.list }}; do echo "$f"; done` | **none** |

So the `${{ }}` interpolation class is **entirely uncovered** by this gate. That class is the
dominant quoting and injection hazard in `release.yml` and `wheels.yml` — e.g.
`wheels.yml:180` is `run: rustup target add ${{ matrix.rustup_target }} ${{ matrix.extra_target }}`,
which shellcheck cannot judge.

Three consequences, all folded into this spec rather than left implicit:

1. §1.2's "0 findings" is a statement about a corpus with this class removed from view.
2. §2.3's rejection of extraction loses one of its three reasons — extraction converts `${{ }}`
   into `$VAR`, which shellcheck *can* judge, so extraction **would** add coverage. §2.3 is
   restated honestly and still rejects it, on the remaining reason.
3. The tool for this class is zizmor (template-injection audit), which CLAUDE.md records as named
   in prose but running nowhere. Out of scope here; recorded as L8 and worth its own issue.

---

## 2. Half A — sourcing shellcheck

### 2.1 The blocker, re-measured

SMA-525's D2 rejected a proto plugin because shellcheck's release ships no checksums asset. Still
true on 2026-09-02: `v0.11.0` (published 2025-08-04) carries **13 archives and no checksums
asset**.

### 2.2 Decision: `shellcheck-py` from PyPI, resolved through uv

`shellcheck-py` 0.11.0.1 wraps the official `shellcheck` 0.11.0 binary.

* **Wheel path** — prebuilt wheels for `macosx_11_0_arm64`, `macosx_10_9_x86_64`,
  `manylinux_2_17_x86_64`, `win_amd64`; binary bundled; `uv.lock` records a sha256 per wheel.
  **Both supported hosts take this path** — CI is `ubuntu-latest` x86_64, the dev box is macOS
  arm64.
* **sdist path** (linux aarch64/riscv64/armv6hf, which have no wheel) — `uv.lock` records the
  sdist hash, and the sdist's `setup.cfg` drives `setuptools-download` with a **hand-pinned sha256
  per asset**.

The republisher's pins were verified against koalaman's own assets rather than trusted:

| asset | verdict |
|---|---|
| `shellcheck-v0.11.0.darwin.aarch64.tar.xz` | **MATCH** |
| `shellcheck-v0.11.0.linux.x86_64.tar.xz` | **MATCH** |
| `shellcheck-v0.11.0.linux.aarch64.tar.xz` | **MATCH** |

Mechanism verified end to end in a throwaway uv project: `uv lock` produced a lock carrying the
sdist hash and all four wheel hashes, and
`uv run --locked --project <dir> python3 -c 'import shutil; print(shutil.which("shellcheck"))'`
printed `<dir>/.venv/bin/shellcheck` at rc 0, reporting `version: 0.11.0`.

**Two honest gaps, both narrowed from rev 1's overclaim:**

* One supply-chain hop — `shellcheck-py` is a third-party republisher (the canonical pre-commit
  mirror), not koalaman. Three digests verified; `uv.lock` pins artifact and hash.
* `uv.lock` does **not** record build requirements. On the sdist path the `setuptools-download`
  build backend is resolved unpinned and unverified at install time. AC A3 therefore holds for the
  wheel path — which is both supported hosts — and the gap is recorded as L3, not waved away as
  "nothing to justify".

**Version specifier:** `shellcheck-py>=0.11.0.1,<0.12`, a bounded range matching
`py/pyproject.toml`'s own stated convention ("Bounded constraints so a lock regen can't silently
bump rule behavior"); `py/uv.lock` pins the exact resolved version. A bump inside that range
re-opens L1's verification, which is stated in the dep's comment (L1).

### 2.3 Rejected: extracting `run:` blocks into `ci/*.sh`

The issue calls this "may be the best of them". Rejected, on one reason rather than rev 1's three:

**The reason that carries it** — a **648-line** refactor (§1.1) concentrated in `release.yml` and
`wheels.yml`, the credentialed publish paths, where a transcription error is a live-release
incident rather than a red PR. Many blocks also interpolate `${{ }}`, so each needs restructuring
into an argument or env var rather than a mechanical move.

**The reason rev 1 gave that is false** — "it buys nothing that §2.2 does not already buy". §1.8
shows it *would* buy the `${{ }}` class, precisely because converting an expression into a shell
variable is what makes it visible to shellcheck. The cost is still not worth it here, but the
trade is real and is recorded so a future issue can revisit it with the right facts.

### 2.4 Rejected: CI-only install

Reintroduces the host-dependent strictness SMA-525 refused, differing only in that the divergence
is signposted.

---

## 3. Half A — design

### 3.1 No new gate, and no registry obligation

actionlint already owns this integration; SMA-525 switched it off. Turning it on is a change to one
existing gate, so **Half A carries none of the registry obligations**. This was checked against
every pin that could plausibly fire — `ACTIONLINT_SH_CALL_SITES`, `SELF_TASK_EXPECTED_GLOBS["actionlint"]`,
`SELF_SCHEDULED_GATES["actionlint"]`, check 8e, `T_AFFECTED_SMOKE_REQUIRED_INPUTS`, `repo:osv`'s
count control (a `>0` floor, not an exact count), and `repo:version-lockstep` (reads `py/uv.lock`
by package name) — and none is touched.

### 3.2 Changes to `ci/actionlint/run.sh`

* Add `shellcheck-py>=0.11.0.1,<0.12` to `py/pyproject.toml`'s `[dependency-groups] dev`.
* Resolve the binary through `uv run --locked --project py` and assert `[ -x ]` on the result — the
  `ci/release-parity/ecosystems/release-plz.sh` idiom. `--locked` is mandatory for the same reason
  `release_guard_py` carries it: a bare `uv run --project py` re-locks `py/uv.lock` as a side effect.
* **Placement: after the `--self-test` early exit at `:4760`, beside the `command -v actionlint`
  guard at `:5157`.** Not at `:79`. `:79` is top-level code on every path, so it would run once per
  mutant in check 9's fan-out as well as under `--self-test` — the cost §1.4 must not hide, and a
  serialisation point on one `py/.venv` in a gate CLAUDE.md already flags as this repo's flakiest
  concurrency surface.
* Replace `ARGS=(-shellcheck= -pyflakes=)` at `:79` with the resolved path.
* **`-pyflakes=` stays off**, but not for rev 1's stated reason (§9 L4).

### 3.3 Fail closed

If shellcheck cannot be resolved, `run.sh` aborts `infra` (rc 2). It must **never** fall back to
`-shellcheck=`.

The operative property is **never a false green**, and that is the whole point: a silent downgrade
to "strictness is a property of the host" is the failure SMA-525 refused. Rev 1 implied CI treats
rc 2 and rc 1 differently; it does not — to Moon and to CI both are a failed task. The rc 2/rc 1
split buys triage legibility (an infrastructure abort is greppable and distinct from a verdict),
not different CI handling, and is only observable by invoking `ci/actionlint/run.sh` directly.

### 3.4 The self-test goes in check 3, not a 14th table

Rev 1 proposed a 14th `*_self_test`. **That was wrong and is withdrawn.** `run_self_tests` is
called at `:4760`, *before* the actionlint PATH guard at `:5157`, and the comment at `:5155-5157`
states the contract explicitly: *"--self-test never shells out to actionlint, so it must not
infra-exit on a machine that simply doesn't have the binary on PATH yet."* A fixture asserting
"actionlint accepts it but shellcheck rejects it" must invoke actionlint, so a 14th table would
silently break that portability contract. Check 9 compounds it, spawning one mutant per self-test
invocation — 15 actionlint+shellcheck runs instead of one.

**Instead:** add a fixture to **check 3** (`:5210-5268`), where the actionlint-dependent fixtures
already live and check 4 is the healthy control. No `SELF_TEST_COUNT` bump, no mutant
multiplication, no `ci_targets.py` churn, and none of the ungated documentation obligations a 14th
table would have created (`usage()` at `:55-70`, `moon.yml:660-698`, the README timing table,
CLAUDE.md's "currently 13").

---

## 4. Half B — `repo:ruff-ci`

### 4.1 Decision: route through `--project py`, not a dedicated project

The obvious precedent is `ci/workflow-credentials/`, whose `pyproject.toml` argues against
`--project py`: `py/` is a `[tool.uv.workspace]` root whose member `paigasus-kernel` depends on
`paigasus-py-bindings` by path, built with maturin — so `uv run --project py` compiles a PyO3
cdylib.

Rev 1 rejected that precedent by claiming "the cdylib cost is paid before `repo:ruff-ci` is
reached". **That claim is false** — neither task declares `deps`, so Moon imposes no ordering and
either may run first or concurrently.

**The sound argument, which survives:** `repo:actionlint`'s `inputs: ['**/*']` strictly supersets
`repo:ruff-ci`'s, so `repo:ruff-ci` can never be a cache miss while `repo:actionlint` is a hit. In
any CI run where this gate does work, `repo:actionlint` is also doing work, and the `py`
environment is materialised **once per run** by whichever task reaches it first. The marginal cost
is zero in aggregate; it is simply not attributable to a particular task.

Locally the cost is honest and stated: `moon run repo:ruff-ci` alone in a fresh worktree pays the
maturin build itself, and CLAUDE.md's worktree-provisioning bullet already covers the remedy.

What this buys over a dedicated project: **no second lockfile**, so no second ruff version, so no
drift between what `py:lint` runs and what `repo:ruff-ci` runs — and therefore no
`repo:version-lockstep`-shaped assertion invented purely to hold that invariant. AC B5 is then true
at both the rule-set and tool-version level, from one file.

Warm, for the record: `uv run --project py --locked ruff check … ci/` is 0.235s;
`uv run --project ci/workflow-credentials` is 0.064s.

### 4.2 The check, and its exit-code contract

**Two steps, deliberately.** Resolve first, then invoke:

```
RUFF="$(uv run --locked --project py python3 -c 'import shutil,sys; p=shutil.which("ruff"); sys.exit(1) if not p else print(p)')"
[ -x "$RUFF" ] || die_infra "…"
"$RUFF" check --config py/pyproject.toml -- "${FILES[@]}"
```

Rev 1 specified `uv run … ruff check …` as one command. **That conflates two failure modes.**
`ruff check` exits 1 on violations; `uv` also exits 1 on a failed resolution and on `--locked`
finding `py/uv.lock` stale. CLAUDE.md records this exact lesson for `repo:workflow-credentials`:
*"`uv` itself exits 1 on a failed resolution, so a shared code would let a PyPI outage read as 'a
workflow declares a credential'."* Resolving the binary first and invoking it directly makes rc 1
unambiguously ruff's; every other status routes to 2.

This matters more than it looks: `.moon/tasks/python.yml:26` runs a bare, **re-locking**
`uv run ruff check .`, so `py/uv.lock` can legitimately be stale in a working tree. Without the
split, that would red `repo:ruff-ci` as though `ci/` were dirty, and a contributor would "fix" it
by re-locking.

**CWD is pinned** with `cd "$(git rev-parse --show-toplevel)"`, as `ci/actionlint/run.sh:30` does:
`--config` resolves relative to CWD, and ruff resolves `src`/`exclude` relative to the config's
directory, so an unpinned CWD gives different answers from different directories.

### 4.3 Corpus derivation replaces the liveness assertion

Rev 1 asserted, after the fact, that the set ruff inspected equals the tracked corpus. Rev 2
**inverts it**: derive the corpus with git and pass it to ruff explicitly, so the equality is
structural rather than asserted.

```
mapfile -t FILES < <(git ls-files -- ':(glob)ci/**/*.py' 'ci/*.py' | sort)
[ "${#FILES[@]}" -ge 10 ] || die_assert "corpus collapsed to ${#FILES[@]} files (floor 10)"
```

Two facts forced this, both measured:

* **`git ls-files 'ci/**/*.py'` does not mean what rev 1 assumed.** Without `:(glob)` magic git
  matches without `FNM_PATHNAME`, so `**` is just two `*`s and the literal `/` is still required:
  it matches `ci/pyo3-stub/check.py` but **not** a top-level `ci/foo.py`. Moon's matcher and
  Python's `glob(recursive=True)` both *do* match `ci/foo.py` for the same pattern, so the gate's
  declared input would schedule it for a file its own corpus derivation could not see.

  **Rev 2 correction (all six forms measured).** Rev 1 added the `'ci/*.py'` companion and claimed
  it "is not redundant with the first". That is backwards, and the two pathspecs are in fact
  *mutually* redundant — each alone is sufficient:

  | pathspec | matches a top-level `ci/foo.py`? |
  |---|---|
  | `'ci/*.py'` (no magic) | **yes** — `*` spans `/`, so it matches at every depth |
  | `':(glob)ci/**/*.py'` | **yes** — under `:(glob)`, `**/` matches zero directories |
  | `'ci/**/*.py'` (no magic) | **no** — the one broken form |

  Both are kept, because the explicit pair documents the intent and costs nothing. What the
  self-test row actually guards is a reduction to the bare `'ci/**/*.py'` — the likeliest
  "simplification" — not the dropping of `:(glob)`, which changes nothing.
* Rev 1's premise that ruff cannot report its inspected set was **wrong** — `ruff check --show-files`
  exists in 0.16.5 and, filtered to `*.py`, returns a set identical to the tracked corpus. The
  inversion is adopted anyway because structural equality cannot drift, and because `--show-files`
  also emits `pyproject.toml` files (ruff lints those for `RUF200`), which rev 1's stated equality
  would have failed on immediately.

The `-ge 10` floor is what stops a silent emptying — the SMA-553 failure class, which
`repo:input-liveness` cannot reach here because `task_inputs.py`'s `_repo_tasks` is keyed to
`projects.get("repo")` by exact project id and proves only that *declared inputs* are live.

### 4.4 Modes and shape

`ci/ruff/run.sh` follows `repo:workflow-credentials`: `--self-test`, `--negative-control`, then the
real run, all four lines under an explicit `set -euo pipefail` in one `script:` block, because Moon
does not enable errexit for `script:` blocks and takes the block's status from its last command.

* **`--self-test`** exercises the corpus-derivation function against synthetic trees (a `mktemp -d`
  git repo): a top-level `ci/foo.py` **must** be found (the §4.3 pathspec trap), a file under
  `ci/x/.venv/` must not, a non-`.py` file must not, and an empty corpus must trip the floor. This
  is the logic worth testing; it needs no ruff and no network.
* **`--negative-control`** plants a known violation and asserts the gate reports **rc 1**. It runs
  against a **copy of the real tree inside the repo** (a temp dir under the worktree, cleaned up),
  not a bare `mktemp -d`: outside a git repo `git ls-files` cannot run and ruff's gitignore
  handling changes, so a bare tempdir would exercise a different code path than the real run.

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
      - '.moon/tasks/python.yml'
      - '.prototools'
      - 'ci/**/*.py'
      - 'ci/ruff/**/*'
      - 'py/pyproject.toml'
      - 'py/uv.lock'
```

`py/pyproject.toml` is the rule set and `py/uv.lock` pins the ruff version; without them a rule
change or a ruff bump leaves a cached PASS standing. `.prototools` pins `uv`. `.moon/tasks/python.yml`
is listed so that a change to how `py:lint` invokes ruff re-keys this gate — AC B5 is "one rule set
*and* one tool version", and that file is where the two could silently diverge.

`inputs` are written **glob-sorted**, because `check_gate_inputs` (`ci_targets.py:1408-1423`)
compares sorted, not authored, order — `SELF_TASK_EXPECTED_GLOBS` must match.

Note `'ci/**/*.py'` formally reaches under `**/.venv/**`, which `.moon/workspace.yml:41-43` asks
tasks not to declare. It is benign — the hasher's `ignorePatterns` filters those trees and ruff's
default `exclude` skips them — and is noted here so the next reader need not re-derive it.

---

## 5. Baseline cleanup

### 5.1 Fix all 27; the exemption hatch exists and ships empty

No `per-file-ignores` are used. But rev 1 *banned* them outright, which contradicts this repo's own
idiom — `T_EXEMPT`, `ALLOW_DEAD_INPUT`, `BRANCH_SKIP`, `COE_SKIP`, `ALLOW_UNLOCKED_CARGO` and
others all ship a reasoned exemption table. Rev 2 defines one, `RUFF_PER_FILE_IGNORE_REASONS`,
requiring a reason string per entry, and **ships it empty — that is the point**, the wording
`T_EXEMPT` uses at `ci_targets.py:129`. Without it, the first legitimate exception (a fixture file
that must contain bad style) forces either a hack or a spec amendment.

Blast radius, checked before committing:

* **`N818` × 3** — `Refused`, `Inconclusive`, `AssertionFailure`. In `run.sh` files they appear
  **only inside comments** (`ci/release-plan/run.sh:156`, `ci/workflow-credentials/run.sh:65`), and
  no whole-line pin array contains them. `ci/release-plan/release_plan.py:221,492` do print
  `type(exc).__name__`, so the rename **is** operator-visible — it is safe because the only
  programmatic consumer greps a different token (`^nothing_to_release=(true|false)$`), not because
  it is invisible. `ci/pyo3-stub/README.md`'s "Refused" vocabulary needs updating too.
* **`E402` × 5** — `workflow_credentials.py`. No runtime reason: `_StrictLoader` at `:63` is the
  only ordering constraint and moving imports up satisfies it. Safe.
* **`RUF100` × 6** — `# noqa: BLE001` where `BLE001` is not in `py/`'s select list. Ruff's autofix
  deletes the whole comment. **Four** carry prose to preserve; `categories.py:332` and `:347` are
  bare and simply go.
* **`UP017` × 3 — not mechanical (§5.3).**
* The remaining 10 (`RUF005`, `UP031`, `UP014`, `N806`, `B904`, `C420`, `SIM300`) are mechanical.

### 5.2 `ruff format` is deliberately not gated (AC B7)

**Decision: `check` only**, recorded here, in `ci/ruff/README.md`, and in CLAUDE.md.

**The reason: 2,998 of ~15,600 existing lines rewritten** (§1.6), driven by `line-length = 200`
joining hand-wrapped lines and collapsing hand-aligned fixture tables. These files are roughly 60%
comment by design; the wrapping is a readability decision, not an accident. Gating it would bury
this PR's actual logic under that diff.

Rev 1 gave a second reason — that formatting could corrupt whole-line pin arrays built from
implicitly concatenated string literals. **That reason was unsupported and is withdrawn.** Parsing
`ci_targets.py` shows every pin array (`RUN_SH_CALL_SITES`, `SELF_SCHEDULED_GATES`,
`ACTIONLINT_SH_CALL_SITES`, `ACTIONLINT_SH_INDENTED_CALL_SITES`, `RELEASE_PARITY_SH_CALL_SITES`,
`WORKFLOW_CREDENTIALS_SH_CALL_SITES`, `RELEASE_PLAN_SH_CALL_SITES`, `SELF_TASK_EXPECTED_GLOBS`) to
be built from single string literals; implicit concatenation appears only in reason and message
strings, which nothing pins. Ruff's stable formatter also never splits a string literal.

This is a decision not to gate formatting, not a claim the corpus is well formatted.

### 5.3 `UP017` raises an interpreter floor — fixed, not exempted

`datetime.UTC` is 3.11+. `ci/publish-metadata/categories.py` is invoked as **bare `python3`**
(`ci/publish-metadata/run.sh:1062,1734,1737`) — the *system* interpreter, since the task is
`toolchain: 'system'` and the file is stdlib-only. CI's `ubuntu-latest` ships 3.12 and stays green;
a contributor on Ubuntu 22.04 (3.10) or macOS Xcode python3 (3.9) would get an `AttributeError`
with no CI coverage.

**Resolution: take the `UP017` fix and add an explicit interpreter-floor preflight** to
`categories.py` — a `sys.version_info < (3, 12)` check exiting with the file's infrastructure code
and a message naming the requirement. Floor 3.12, matching `py/pyproject.toml`'s
`target-version = "py312"` and the `--python '>=3.12'` the other `ci/` gates already require. This
keeps §5.1's no-carve-out result while turning a silent `AttributeError` into a stated requirement.

The general form is recorded as L7: importing a `py312` target-version into a corpus executed by an
unpinned system interpreter means every future `UP` rule ratchets that floor, and only the
preflight makes the ratchet visible.

---

## 6. Proof each gate can red (AC C9)

Five mutations. **None has been performed yet** — rev 1 wrote this section in the past tense for
work that had not happened. The plan must perform each and record its output.

| # | mutation | expected |
|---|---|---|
| M1 | plant an `SC2086` (shell variable, not `${{ }}`) in a real workflow `run:` | `repo:actionlint` **rc 1** |
| M2 | make shellcheck unresolvable | `ci/actionlint/run.sh` **rc 2**, never green |
| M3 | plant a `RUF005` in a `ci/**/*.py` file | `repo:ruff-ci` **rc 1** |
| M4 | make `py/uv.lock` stale / uv resolution fail | `repo:ruff-ci` **rc 2**, not rc 1 (§4.2) |
| M5 | remove any one of the six registrations | `repo:affected-smoke` **rc 1** |

M2 and M4 are the ones a "does it work?" check would skip, and they are the two that prove the
fail-closed contracts. **Both must be measured by invoking the `run.sh` directly, not through
`moon run`** — Moon reports its own status, so rc 2 vs rc 1 is not observable through it.

M1 must use a shell variable: a `${{ }}` fixture would pass and prove nothing (§1.8).

`--self-test` and `--negative-control` make M1/M3 permanent rather than one-time.

---

## 7. Registry wiring (AC C8) — six obligations, not five

Half A adds none (§3.1). `repo:ruff-ci` pays six; the issue's list of five omits the last.

1. **`ci.yml`'s `T=(…)`** — append `:ruff-ci`, keeping it a single-line bash array (SMA-541).
2. **CLAUDE.md's marker-delimited command** — the same target between the `ci-targets` markers.
3. **`SELF_SCHEDULED_GATES`** — its four `moon.yml` lines, matched as whole lines after stripping.
4. **`SELF_TASK_EXPECTED_GLOBS`** — all six `inputs` from §4.4, **glob-sorted**.
5. **`RUFF_SH_CALL_SITES`** — a new script pin. Rev 1 declined this; that was wrong.
   `SELF_SCHEDULED_GATES` pins only the four `moon.yml` lines, leaving the control's *body* in
   `run.sh` unpinned, and CLAUDE.md records the measured outcome for this identical shape:
   *"deleting every `_expect` and `grep` row left all four byte-identical and the control exited 0
   having asserted nothing (MEASURED)"* — the same lesson `RELEASE_PARITY_SH_CALL_SITES` (two
   measured bypasses) and `T_CARGO_LOCK_SH_CALL_SITES` encode. Pin the flag parse, the control's
   assertion line, both report arms, the corpus-derivation line, and the real ruff invocation
   including `--config`.
6. **`REQUIRED_REPO_TASKS`** (`ci_targets.py:156-168`) — the floor that stops a gate being switched
   off by dropping it from `T` and making it CI-ineligible in one edit. Its `workflow-credentials`
   entry gives the reason verbatim: *"this gate carries a `--negative-control` … so without a floor
   entry the whole gate — control included — could be switched off with every check green."*
   `repo:ruff-ci` carries a negative control and hits that reasoning exactly. (`pyo3-stub-drift` is
   absent from the floor, so precedent is mixed; this spec follows the reasoned entry.)

**Reachability:** `ci/**/*` is already declared (`moon.yml:205`) and floored
(`ci/actionlint/run.sh:2123`), so obligation 5 is reachable without a new floor entry. But
SMA-530, SMA-593 and SMA-603 each *also* added a narrow sibling glob, and the comment at
`moon.yml:200-204` explains why they are kept rather than collapsed: check 8e floors the array at
`-ge 20` against 23 entries, so headroom is deliberate. Add `ci/ruff/**/*` and
`T_AFFECTED_SMOKE_REQUIRED_INPUTS` alongside, following the three predecessors.

`SELF_TASK_GLOBS_EXEMPT` is **not** used — holding both it and `SELF_TASK_EXPECTED_GLOBS` is itself
reported.

---

## 8. Files touched

| file | change |
|---|---|
| `py/pyproject.toml` | `shellcheck-py>=0.11.0.1,<0.12` in the dev group, with the L1 comment |
| `py/uv.lock` | regenerated |
| `ci/actionlint/run.sh` | resolve shellcheck after `:4760`; `ARGS` at `:79`; check-3 fixture |
| `ci/actionlint/README.md` | integrity story (AC A2), the `${{ }}` and `SC2148`/`SC2164` limits |
| `ci/ruff/run.sh` `README.md` | new gate, three modes |
| `moon.yml` | `repo:ruff-ci` task; `ci/ruff/**/*` on `repo:affected-smoke` |
| `.github/workflows/ci.yml` | `:ruff-ci` in `T=(…)` |
| `ci/affected-graph/ci_targets.py` | 4 registry entries + `RUFF_SH_CALL_SITES` |
| `ci/actionlint/run.sh` | `T_AFFECTED_SMOKE_REQUIRED_INPUTS` += `ci/ruff/**/*` |
| 6 × `ci/**/*.py` | the 27 fixes; `categories.py` also gets the §5.3 preflight |
| `ci/pyo3-stub/README.md`, `ci/release-plan/README.md`, `ci/workflow-credentials/README.md` | N818 vocabulary |
| `CLAUDE.md` | new gate's Gotchas entry; the format decision; the `${{ }}` limit |

---

## 9. Non-goals and limitations

* **L1 — one supply-chain hop.** `shellcheck-py` is a third-party republisher. Three digests were
  verified against koalaman's assets (§2.2); a version bump re-opens that and must re-verify. The
  bounded specifier means Dependabot will propose bumps; the obligation is stated in the dep's
  comment, and nothing machine-checks it.
* **L2 — `SC2148` and `SC2164` cannot fire.** Structural, not configured (§1.3).
* **L3 — the sdist path's build dependency is unpinned.** `uv.lock` does not record build
  requirements, so `setuptools-download` is resolved unverified at install time on linux
  aarch64/riscv64/armv6hf. Both supported hosts take the wheel path, where AC A3 holds fully.
* **L4 — `-pyflakes=` stays off, and inline Python is uncovered.** Rev 1 claimed no `run:` block
  executes inline Python. **False:** `wheels.yml:233-254` and `:262` run real Python programs via
  `python - <<'PY'` heredocs. The true reason pyflakes would not help is that actionlint applies it
  only to steps declaring `shell: python`, and a bash heredoc is invisible to it regardless. Those
  programs are linted by neither half of this issue and are outside `repo:ruff-ci`'s `ci/**/*.py`
  corpus.
* **L5 — formatting is ungated by decision** (§5.2).
* **L6 — `repo:ruff-ci` covers `ci/` only.** No tracked Python exists outside `py/` and `ci/` today
  (measured: zero files), but nothing asserts that partition; Python at a third location would be
  unlinted by both. The `wheels.yml` heredocs of L4 are the live instance.
* **L7 — the `UP` rules ratchet an interpreter floor.** §5.3 fixes today's instance and makes the
  floor explicit for `categories.py`; the other bare-`python3` gate scripts have no such preflight,
  so a future `UP` rule can raise their floor with no CI coverage.
* **L8 — the `${{ }}` class is uncovered** (§1.8). zizmor is the tool for it and runs nowhere in
  this repo. Worth its own issue.
* **L9 — isort first-party classification is wrong for `ci/`.** Ruff's `src` defaults to the config
  directory (`py/`), so a future `from ci_targets import …` inside `ci/affected-graph/` would be
  classified third-party and `I001` would enforce the wrong order. No `ci/` file imports a sibling
  today (verified across all ten).
* **L10 — concurrent `uv` behaviour is unmeasured.** `py:lint` runs a bare re-locking
  `uv run`, while `repo:actionlint`, `repo:ruff-ci` and `ci/release-plan` all run `--locked`; one
  `moon ci` can schedule these concurrently. §4.2's split makes a stale-lock failure *legible*
  (rc 2, not a false lint red), but whether uv's venv locking is sufficient for three-plus
  concurrent `uv run --project py` invocations has not been measured here.

---

## 10. Changelog — rev 1 → rev 2

Reworked after an adversarial challenge. Every item below was independently verified against the
repo before folding in; the challenge's one incorrect premise is recorded too.

**Blockers fixed**

1. **§4.3 liveness inverted.** Now derives the corpus with `git ls-files` and passes an explicit
   file list, with a `-ge 10` floor. (The challenge's premise — that ruff cannot report its
   inspected set — was **wrong**: `--show-files` exists in 0.16.5 and was verified to match the
   corpus. The inversion is adopted anyway: structural equality cannot drift, and `--show-files`
   also emits `pyproject.toml` files, which rev 1's stated equality would have failed on.)
2. **`git ls-files 'ci/**/*.py'` pathspec trap** — and, amended during implementation, the
   *reason* for the companion pathspec. All six forms are now measured in §4.3: the two pathspecs
   are mutually redundant, each alone sufficient, and only the bare `'ci/**/*.py'` is broken. The
   claim that the companion "is not redundant with the first" was backwards.
   The original finding stands: Verified by probe: it misses a top-level
   `ci/foo.py` that Moon's own matcher would schedule the gate for. Now `':(glob)ci/**/*.py' 'ci/*.py'`.
3. **Half B exit-code disambiguation.** Resolve the ruff binary, then invoke it directly, so rc 1
   is unambiguously ruff's and a uv/PyPI failure cannot read as "the CI tooling has lint
   violations". Rev 1 applied fail-closed reasoning to Half A only.
4. **The 14th self-test is withdrawn.** Verified at `ci/actionlint/run.sh:4760` and `:5155-5157`:
   `run_self_tests` runs *before* the actionlint PATH guard, by documented design, so an
   actionlint-invoking fixture would break `--self-test`'s portability contract and multiply
   through check 9's mutant fan-out. The fixture moves to check 3.

**Corrections to rev 1's own measurements**

5. **§1.6's "5071 changed lines" double-counted** both sides of the diff. The honest figure is
   2,998 existing lines rewritten.
6. **§5.2's pin-corruption reason was unsupported** and is withdrawn — parsing `ci_targets.py`
   shows no pin array is built from implicit string concatenation. §5.2 now rests on churn alone.
7. **§1.2's "the corpus is genuinely clean" is qualified.** §1.8 is new: measured, `${{ }}`
   expressions are replaced with inert placeholders before shellcheck sees them, so that entire
   class is uncovered. §2.3 reason 3 ("extraction buys nothing") was false and is withdrawn;
   extraction is still rejected, on transcription risk alone.
8. **§4.1's ordering claim was false.** Moon imposes no ordering between the two tasks. Replaced
   with the superset argument, which is sound, plus an honest statement of the local cost.
9. **L4 was factually wrong** — `wheels.yml` does run inline Python in heredocs.
10. **§2.2 overstated "every path checksummed"** — build requirements are not in `uv.lock` (L3).
11. **§3.3 implied CI distinguishes rc 2 from rc 1.** It does not; the property is "never a false
    green", and rc 2 buys triage legibility only.
12. **§6 was written in the past tense** for unperformed work, and omitted that M2/M4 cannot be
    measured through `moon run`.

**Scope additions**

13. **A sixth registry obligation** — `REQUIRED_REPO_TASKS`, whose `workflow-credentials` entry
    states reasoning that applies verbatim to any gate carrying a negative control.
14. **`RUFF_SH_CALL_SITES`** — rev 1 declined a script pin; CLAUDE.md records a measured bypass for
    exactly that omission. Plus `ci/ruff/**/*` as a narrow glob, following SMA-530/593/603.
15. **§5.3** — `UP017` raises `categories.py`'s interpreter floor to 3.11 under a bare `python3`.
    Fixed with an explicit 3.12 preflight rather than an exemption.
16. **An exemption hatch that ships empty** (§5.1) — rev 1 banned `per-file-ignores` outright,
    contradicting the repo's own idiom.
17. **`--self-test` and `--negative-control` contents are now specified** (§4.4), including why the
    control must not run in a bare `mktemp -d`.
18. **Minor:** CWD pinning; glob-sorted `inputs`; `.moon/tasks/python.yml` as an input; bounded
    version specifier; `.venv` exclusion attributed to ruff's default `exclude`; §1.3 led with the
    empirical probe; AC numbering unified on the A/B/C scheme; L9/L10 added.

---

## 11. Acceptance criteria mapping

| AC | where |
|---|---|
| **A1** inline bash linted deterministically, same on dev box and CI | §2.2, §3.2 — one hash-pinned binary from a committed lockfile |
| **A2** integrity story written where the next person finds it | §2.2, plus `ci/actionlint/README.md` and the dep comment |
| **A3** no unchecksummed download without explicit justification | §2.2 — holds fully on the wheel path (both supported hosts); the sdist build-dep gap is stated as L3 |
| **B4** a Ruff violation under `ci/` fails CI | §4.2, proven by M3 (§6) |
| **B5** same rule set as `py/`, not a second copy | §4.1, §4.2 — `--config py/pyproject.toml`, one ruff version, `.moon/tasks/python.yml` as an input |
| **B6** every existing `ci/**/*.py` passes on an unmutated tree | §5.1, §5.3 — all 27 fixed from the fresh §1.5 baseline |
| **B7** `ruff format` decided explicitly and recorded | §5.2 — check-only, on the corrected 2,998-line measurement |
| **C8** registry obligations, or `:affected-smoke` reds | §7 — six, not the issue's five |
| **C9** each gate proven to red by mutation | §6 — M1-M5, none yet performed; `--self-test`/`--negative-control` make M1/M3 permanent |
