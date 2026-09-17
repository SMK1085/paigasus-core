# SMA-638 — pin the tailwind-guard check's call site, and one fail-closed papercut

Date: 2026-09-17
Issue: [SMA-638](https://linear.app/smaschek/issue/SMA-638/ci-pin-the-tailwind-guard-checks-call-site-and-two-fail-closed)
Related: SMA-512 (which added the exposure), SMA-553 (which set the rc-1 / rc-2 precedent)

## 1. Scope

The issue lists three items. Two of them are already on `main`. This work delivers the
other two changes.

| Issue item | State on `main` | Evidence |
|---|---|---|
| 1. Nothing pins `check_tailwind_guard_invocations`' call site | **Open** | No file outside `ci/affected-graph/ci_targets.py` names the function. Nothing pins any line of that file's own `main()`. |
| 1, "Related" note. No self-test row drives the `registry=None` default | **Done** | `ci_targets.py:3241-3250`, added by `43afaf4a` (SMA-512 pull request 3). |
| 2. A non-string `script` is misreported as `no_project` | **Done** | `ci_targets.py:1946-1953` raises `MoonOutputError`. Added by `2ff37313` (SMA-512 pull request 1). |
| 3. An empty `dirs` array prints a blank row | **Open** | `ci/next-env/run.sh:141`. |

The two open items are independent. They share this specification because they share an
issue, not because one depends on the other.

## 2. Problem 1 — `main()` can drop a check and stay green

`ci_targets.py` defines eleven `check_*` functions. `main()` wires each one three times:

1. it calls the function and binds the result names,
2. it reads those names in the pass-branch condition that decides the exit code,
3. it renders those names as rows in the report tuple.

`self_test()` drives each `check_*` function directly, against synthetic fixtures. No
assertion connects the functions to `main()`. So an edit that deletes a call and its
condition terms removes the check from every CI run, and `--self-test` still passes.

The measurement in the issue is correct. A reviewer confirmed it for
`check_tailwind_guard_invocations`. The exposure is not specific to that check. It applies
to all eleven.

### 2.1 The one existing control, and why it does not generalize

`ci_targets.py:3107-3123` already reads `main()`'s own source:

```python
main_src = inspect.getsource(main)
for _name in (
    "pairing_unpinned", "pairing_bad_exempt", "pairing_stale_exempt", "pairing_both",
    "pairing_orphan_globs",
):
    _count = main_src.count(_name)
    if _count < 3:
```

This covers five names of twenty-three. It is a hand-written list, so a new check does not
join it automatically.

It also cannot be widened as written. The test is a **substring count**. Three of the names
now in scope are substrings of other names in the same function: `missing` occurs inside
`missing_sites`, `floor` occurs inside `check_floor`, and `dead` occurs inside prose in the
same source. A textual generalization would therefore pass without asserting anything. The
replacement must resolve names exactly.

## 3. Design 1 — `main_wiring_findings`

Add one pure function to `ci/affected-graph/ci_targets.py`.

```python
def main_wiring_findings(main_source, check_names, exempt=None):
    """Returns (uncalled, unconsumed, unreported), all sorted."""
```

It parses `main_source` with `ast`. It returns three sorted lists of findings:

| Finding | Meaning |
|---|---|
| `uncalled` | A name in `check_names` that `main()` never calls. |
| `unconsumed` | A name bound by a `check_*` call that the pass-branch condition never reads. |
| `unreported` | A name bound by a `check_*` call that no report tuple renders. |

The function resolves every name through the syntax tree. It never counts substrings.

### 3.1 How it finds each of the three wires

- **The calls.** It walks the `main` function definition for an `Assign` node whose value is
  a `Call` to a `Name` in `check_names`. It collects the assignment targets. A target is
  either one `Name` or a `Tuple` of `Name` nodes.
- **The pass-branch condition.** It selects the `If` node whose body returns the constant
  `0`. That is the branch that reports a pass. It collects every `Name` the test loads.
- **The report tuple.** It selects each `For` node whose iterable is a `Tuple`. It collects
  every `Name` loaded inside that iterable.

A bound name must appear in the condition set and in the report set. A `check_names` entry
must appear among the called functions.

The function does **not** discover `check_names` itself. The caller passes it. This keeps the
function pure, so the self-test can drive it with a synthetic source and a synthetic name set
that share no state with the live module. `self_test()` derives the live set from the module's
own globals: every value that is a function, whose `__name__` starts with `check_`, and whose
`__module__` is this module.

The name `main_wiring_findings` carries **no** `check_` prefix, and that is deliberate. A
function named `check_main_wiring` would be a `check_*` function that `main()` must never
call — a direct contradiction of the rule this same function enforces. It would need a
permanent carve-out in the derivation, and that carve-out would be the one place a future
self-test-only check could hide. The name removes the contradiction instead of excusing it,
and it keeps the rule exceptionless: every `check_*` function is called by `main()`, with an
empty `exempt` and no hidden exclusion.

### 3.2 Measured state of the repository today

A probe over the current `main` gives:

- eleven `check_*` functions defined, and eleven called by `main()`,
- twenty-three bound result names, and twenty-three present in the pass-branch condition,
- twenty-three present in a report tuple.

All three finding lists are therefore empty today. `exempt` ships **empty**. Every exclusion
must be structural, which is the rule `cargo_moon_parity.py`'s A10 already follows.

The earlier concern that `check_registry_pairing` would need an exemption was wrong. `main()`
does call it. The probe proves this.

### 3.3 Where the call lives, and why

`self_test()` calls `main_wiring_findings`. `main()` does not.

Three reasons.

- The guard asserts a property of the source text. It does not assert a property of the
  repository's task graph. A negative-control path is the correct home for such an assertion.
- `main()` shells out to `moon query tasks`. The guard is pure and needs no such call.
- The line that runs `self_test()` is already pinned twice, across two files.

The third reason is the load-bearing one. `repo:affected-smoke` runs this script:

```
set -euo pipefail
ci/affected-graph/run.sh --negative-control
ci/affected-graph/run.sh
```

`--negative-control` reaches `ci/affected-graph/run.sh:760`:

```
python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1
```

That exact line is pinned by `RUN_SH_CALL_SITES` inside `ci_targets.py`, and again by
`T_AFFECTED_GRAPH_CALL_SITES` inside `ci/actionlint/run.sh`. `repo:actionlint` carries
`inputs: ['**/*']`, so it is scheduled independently of `repo:affected-smoke`. A deletion in
one file therefore reds a gate in the other.

This is what makes a new `ci/actionlint/run.sh` check unnecessary. The cross-gate pin that
such a check would add already exists. The work needs **no** new actionlint check, **no**
`SELF_TEST_COUNT` bump, and **no** mutation-battery entry.

### 3.4 What `main_wiring_findings` replaces

The block at `ci_targets.py:3107-3123` is deleted. `main_wiring_findings` covers the five
`pairing_*` names it asserted, and eighteen more, and it resolves them exactly rather than by
substring count. Keeping both would leave two controls for one property, one of them weaker
and hand-maintained.

### 3.5 Negative controls

`self_test()` drives `main_wiring_findings` against synthetic sources it fully controls, and
then once against the real `main()`.

| Row | Fixture | Expected finding |
|---|---|---|
| `compliant` | A synthetic `main` that wires one check three times. | `([], [], [])` |
| `uncalled` | The same source, with the call deleted. | one `uncalled` row |
| `unconsumed` | The same source, with the name dropped from the condition. | one `unconsumed` row |
| `unreported` | The same source, with the name dropped from the report tuple. | one `unreported` row |
| `tuple unpack` | A synthetic `main` binding a four-name tuple, wired correctly. | `([], [], [])` |
| `partial unpack` | The same, with one of the four names dropped from the condition. | one `unconsumed` row |
| live | `inspect.getsource(main)` and the module's real `check_*` set. | `([], [], [])` |

The `partial unpack` row matters most. It reproduces the exact shape the issue reports: a
tuple-unpacking call whose terms are removed one at a time.

The live row is the row that would have caught SMA-512's exposure. It also holds the
`exempt` default honest, the same way `ci_targets.py:3241-3250` holds
`check_tailwind_guard_invocations`' `registry=None` default honest.

## 4. Problem 3, and a correction to the issue

`ci/next-env/run.sh:141` reads:

```bash
for d in "${dirs[@]:-}"; do
```

When `dirs` is empty, this loop runs **once**, with `d` set to the empty string. The gate
then exits 2 and prints a blank line under the heading "these `ts/apps/*` directories have no
discoverable next.config.\*". The behaviour is measured.

**The issue's reachability claim is wrong.** It says the case is "reachable only when some
`ts/apps/*/next.config.*` exists and no `ts/apps/*/package.json` does". That state cannot
reach line 141.

- `apps` needs a `next.config.*` **and** a `package.json` (`run.sh:119-122`).
- `run.sh:125-128` exits 2 when `apps` is empty.
- `dirs` needs only a `package.json` (`run.sh:136-138`), which is a weaker condition.

Every directory that qualifies for `apps` therefore also qualifies for `dirs`. Reaching line
141 proves `apps` is non-empty, which proves `dirs` is non-empty. The blank row is
unreachable under the current control flow.

The fix still lands. It is defensive hardening against a future reordering of the two blocks,
not the repair of a live fault. The specification records this so that a later reader does not
credit the change with more than it does.

### 4.1 Design 3

Guard the loop on the array length instead of using the `:-` idiom:

```bash
if [ "${#dirs[@]}" -gt 0 ]; then
  for d in "${dirs[@]}"; do
    ...
  done
fi
```

This form is safe under `set -u` on bash 3.2, which is the shell `repo:affected-smoke` needs
on a local machine. The issue names this same form.

### 4.2 A second occurrence, deliberately left alone

`ci/next-env/run.sh:36` uses the same idiom:

```bash
for f in "${RESTORE_FILES[@]:-}"; do
```

Here the empty case **is** reachable. `RESTORE_FILES` is empty when the `EXIT` trap fires on
either early `exit 2` path. The loop is harmless, because line 37 guards the body:

```bash
[ -n "$f" ] || continue
```

This work leaves that line unchanged. It is correct as written, and editing it serves no
assertion in scope.

## 5. Non-goals

- No new check in `ci/actionlint/run.sh`. Section 3.3 gives the reason.
- No `--self-test` or `--negative-control` mode for `ci/next-env/run.sh`. That gate declines
  both deliberately, and `run.sh:105-107` records the cost in registry entries.
- No completeness assertion on `repo:next-env-drift`'s hand-written `deps` list. That gap is
  real and recorded in CLAUDE.md. It is a separate piece of work.
- No change to `check_tailwind_guard_invocations` itself. Its own fixtures already pass.

## 6. Residual risk

**A single edit that deletes `main_wiring_findings`'s call from `self_test()` is a silent
green.** Nothing catches it.

Adding another pin does not close this. The new pin would itself need a pin. The regress moves
out one level and does not stop. `ci/actionlint/README.md`'s L16 records the same shape for
`ACTIONLINT_SH_CALL_SITES` having no arity floor, and accepts it for the same reason.

What bounds the risk: the deletion must remove a named call from a negative-control function
whose only purpose is to prove assertions fire. Such a deletion is visible in review.

This risk is **narrower than the one the work removes**. Today, eleven checks each carry an
independent silent-green route through `main()`. After this change, one route remains, and it
runs through a function a reviewer reads as a control.

## 7. Files touched

| File | Change |
|---|---|
| `ci/affected-graph/ci_targets.py` | Add `main_wiring_findings`. Add its self-test rows. Delete the `main_src.count(...)` block at lines 3107-3123. |
| `ci/next-env/run.sh` | Guard the `dirs` loop on array length. |
| `docs/superpowers/specs/2026-09-17-sma-638-main-wiring-guard-design.md` | This file. |

`ci/affected-graph/ci_targets.py` is linted by `repo:ruff-ci` against `py/pyproject.toml`'s
rule set. The new code must pass it.

## 8. Verification

| Command | Expected |
|---|---|
| `python3 ci/affected-graph/ci_targets.py --self-test` | rc 0. Section 3.5's four negative rows — `uncalled`, `unconsumed`, `unreported` and `partial unpack` — are what prove it can red. |
| Delete the four `tw_*` terms from `main()`'s pass-branch condition, then re-run `--self-test` | rc 1, naming four `unconsumed` findings. This is the exact regression SMA-638 reports, and it must fail after the change and pass before it. |
| `bash ci/next-env/run.sh` | rc 0. |
| `moon run repo:affected-smoke --force` | Passes. Run it with system `/bin/bash` first on `PATH`. |
| `moon run repo:ruff-ci` | Passes. Needs bash 4 or later, so run it with Homebrew bash. |

Both baselines were measured green before any change: `ci_targets.py --self-test` returned
rc 0, and `ci/next-env/run.sh` returned rc 0.
