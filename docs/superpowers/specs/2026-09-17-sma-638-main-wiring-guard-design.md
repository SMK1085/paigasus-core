# SMA-638 — pin the tailwind-guard check's call site, and one fail-closed papercut

Date: 2026-09-17
Issue: [SMA-638](https://linear.app/smaschek/issue/SMA-638/ci-pin-the-tailwind-guard-checks-call-site-and-two-fail-closed)
Related: SMA-512 (which added the exposure), SMA-542 and SMA-560 (which built the pattern this
work adopts), SMA-599 (which measured that a name-based guard is not enough)

Revision 2. An adversarial review rejected revision 1's design. Section 9 records what changed
and why.

## 1. Scope

The issue lists three items. Two of them are already on `main`. This work delivers the
other two changes.

| Issue item | State on `main` | Evidence |
|---|---|---|
| 1. Nothing pins `check_tailwind_guard_invocations`' call site | **Open** | No file pins any line of `ci/affected-graph/ci_targets.py`'s `main()`. |
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

The three wires are written separately. `self_test()` drives each `check_*` function directly,
against synthetic fixtures. No assertion connects the functions to `main()`. So an edit that
deletes a call and its condition terms removes the check from every CI run, and `--self-test`
still passes.

The issue reports this for `check_tailwind_guard_invocations`. The exposure is not specific to
that check. It applies to all eleven.

### 2.1 The one existing control, and what it does cover

`ci_targets.py:3116-3127` reads `main()`'s own source and requires five named strings to appear
at least three times each:

```python
main_src = inspect.getsource(main)
for _name in (
    "pairing_unpinned", "pairing_bad_exempt", "pairing_stale_exempt", "pairing_both",
    "pairing_orphan_globs",
):
    _count = main_src.count(_name)
    if _count < 3:
```

It covers five names of twenty-three, and it is hand-written, so a new check does not join it
automatically.

It does, however, cover one case worth keeping. The names are **literals**. A count of zero is
below three, so deleting `check_registry_pairing` *entirely* — the function, its call, its five
condition terms and its five report rows — still reds this block. Any replacement that derives
its name set from the live module loses that, because a deleted function simply vanishes from
the set. Section 3.3 keeps the property by other means.

## 3. Design 1 — adopt `collect_findings`, the shape the sibling file already uses

`ci/affected-graph/cargo_moon_parity.py` sits in the same directory and had the identical
problem. It did not add a detector. It removed the possibility.

### 3.1 The pattern

`cargo_moon_parity.py:4233` defines `collect_findings`, which returns a list of
`(key, rows, title)` triples. `main()` then derives **both** wires from that one list:

```python
# cargo_moon_parity.py:4379 — the verdict
if not any(rows for _, rows, _ in findings):
# cargo_moon_parity.py:4392 — the report
for _, rows, title in findings:
```

Its docstring states the reason (`:4236-4239`): "Before SMA-542 the two were written
separately, so a new check folded into one and not the other was a green no-op." The plan that
introduced it is blunter
(`docs/superpowers/plans/2026-08-28-sma-560-cross-stack-input-affectedness.md:493`): "Building
both from one list removes the possibility rather than detecting it."

Under this shape, a check that is collected but not judged, or judged but not reported, cannot
exist. There is one list, and both wires read it.

### 3.2 Applying it to `ci_targets.py`

Restructure `main()`:

- Add a `collect_findings(...)` function that returns the eleven checks' results as
  `(key, rows, title)` triples, in a fixed order.
- Replace the pass-branch condition at `:3565-3570` with
  `if not any(rows for _, rows, _ in findings)`.
- Replace the report tuple at `:3584-3767` with `for _, rows, title in findings`.

The twenty-three result names stop being twenty-three separate wires. They become rows inside
the triples.

This is the largest part of the work. It is a mechanical restructure of one function, and it
makes `ci_targets.py` consistent with the file beside it.

### 3.3 `EXPECTED_FINDING_KEYS` — the membership floor

The restructure alone does not stop a check being removed from the list. `cargo_moon_parity.py`
closes that with a pinned key tuple (`:4230`):

```python
EXPECTED_FINDING_KEYS = ("a1", "a2", "a3", "a4-lint", "a4-fmt", "a5", "a6", "a7", "a8", "a9", "a10")
```

`self_test()` asserts arity first, then the exact key sequence (`:2325-2337`). Its own comment
records why a name-based guard is not enough, and that the finding is **measured**, not
reasoned: a name-based guard "never sees `check` at all (no `check_` prefix), so three measured
deletions from the findings list left `--self-test` green with a real assertion gone".

`ci_targets.py` gains the same pair: an `EXPECTED_FINDING_KEYS` tuple naming its eleven checks'
keys, and a `self_test()` row asserting arity and exact sequence against a fixture. This is what
preserves section 2.1's whole-check-deletion property, and it does so for all eleven checks
rather than for one.

### 3.4 What this replaces

The block at `ci_targets.py:3116-3127` is deleted, and only now is the replacement genuinely
stronger on every axis:

| Failure | Old block | New design |
|---|---|---|
| A name judged but not reported | Caught, for 5 names of 23 | **Impossible** |
| A name reported but not judged | Caught, for 5 names of 23 | **Impossible** |
| A whole check deleted | Caught, for 1 check of 11 | Caught, for all 11, by `EXPECTED_FINDING_KEYS` |
| A check added and wired to neither | Not caught | Caught, by the arity assertion |

### 3.5 Negative controls

`self_test()` drives `collect_findings` against a fixture it controls:

| Row | Fixture | Expected |
|---|---|---|
| arity | A valid payload. | `len(collected) == len(EXPECTED_FINDING_KEYS)` |
| key sequence | The same payload. | `tuple(key for key, _, _ in collected) == EXPECTED_FINDING_KEYS` |
| empty floor | — | `EXPECTED_FINDING_KEYS` is non-empty, or the floor asserts nothing. |

These mirror `cargo_moon_parity.py:2294-2337` row for row. The arity row must be built so that
it fails on arity and not on some check's own violation — the sibling's fixture comments record
that trap, and the same care applies here.

## 4. Design 2 — close the negative-control bypass

Section 3's guard runs on `--self-test`. That path is reachable only if two lines survive, and
**neither is pinned today**:

```bash
ci/affected-graph/run.sh:28   [ "${1-}" = "--negative-control" ] && NEGATIVE=1
ci/affected-graph/run.sh:714  if [ "$NEGATIVE" = 1 ]; then
```

Deleting line 28 leaves `NEGATIVE` at its initialised `0`. The negative-control branch never
runs, `run.sh` falls through to `run_suite`, and the gate exits 0 having run the real suite
twice. Both existing pins stay green, because both pin only these two strings:

```
"assert_ci_targets || SUITE_RC=1"
'"$HERE/ci_targets.py" --self-test || NEG_RC=1'
```

CLAUDE.md records this exact bypass as **measured** for `repo:release-parity`: "neutering the
flag parse (dropping `NEGATIVE=1`) leaves `NEGATIVE` at its initialized 0, so the control branch
is never entered and the invocation falls through to the real suite, which then just runs twice
and proves nothing." That gate closed it by pinning its flag parse and its `NEGATIVE` guard.
`ci/affected-graph/run.sh` never did.

Fix: add both lines to **both** pin tables —

- `RUN_SH_CALL_SITES` in `ci/affected-graph/ci_targets.py:439-449`,
- `T_AFFECTED_GRAPH_CALL_SITES` in `ci/actionlint/run.sh:2051-2054`,

and update `affected_graph_wiring_self_test`'s fixtures (`ci/actionlint/run.sh:3174-3195`) and
`check_self_invocation`'s `wired_run_sh` fixture in the same edit.

This reuses check 8c's existing verdict function and its existing self-test. It needs **no** new
`ci/actionlint/run.sh` check, **no** `SELF_TEST_COUNT` bump, and **no** mutation-battery entry.

## 5. Problem 3, and a correction to the issue

`ci/next-env/run.sh:141` reads:

```bash
for d in "${dirs[@]:-}"; do
```

When `dirs` is empty, this loop runs **once**, with `d` set to the empty string. The gate then
exits 2 and prints a blank line under the heading "these `ts/apps/*` directories have no
discoverable next.config.\*".

**The issue's reachability claim is wrong.** It says the case is "reachable only when some
`ts/apps/*/next.config.*` exists and no `ts/apps/*/package.json` does". That state cannot reach
line 141.

- `apps` needs a `next.config.*` **and** a `package.json` (`run.sh:120-123`).
- `run.sh:126` exits 2 when `apps` is empty.
- `dirs` needs only a `package.json` (`run.sh:137-139`), a strictly weaker test.

Every directory that qualifies for `apps` therefore also qualifies for `dirs`. Reaching line 141
proves `apps` is non-empty, which proves `dirs` is non-empty. The blank row is unreachable under
the current control flow. An independent reviewer confirmed this reading.

The fix still lands. It is defensive hardening against a future reordering of the two blocks,
not the repair of a live fault.

### 5.1 The fix

```bash
if [ "${#dirs[@]}" -gt 0 ]; then
  for d in "${dirs[@]}"; do
    ...
  done
fi
```

This form is safe under `set -u` on bash 3.2. Lines 126 and 148 of the same file already use it.

### 5.2 A second occurrence, deliberately left alone

`ci/next-env/run.sh:36` uses the same idiom, and there the empty case **is** reachable:
`RESTORE_FILES` is empty when the `EXIT` trap fires on either early `exit 2` path. The loop is
harmless, because line 37 guards the body with `[ -n "$f" ] || continue`. This work leaves it
unchanged.

## 6. Documentation to correct

`ci/affected-graph/README.md:350-354` is stale on two facts this work depends on:

- It states `check_registry_pairing` "is not called from `main()`". `main()` does call it
  (`ci_targets.py:3561-3562`).
- It cites the self-test invocation as "run.sh:404". The line is `ci/affected-graph/run.sh:760`.

Both are corrected, and the README gains a short note on the new floor's limits (section 7).

CLAUDE.md gains an entry, because it enumerates the registry obligations for every gate of this
shape.

## 7. Non-goals and stated limits

- **No new check in `ci/actionlint/run.sh`.** Section 4 reuses check 8c.
- **No `--self-test` for `ci/next-env/run.sh`.** That gate declines it deliberately, and
  `run.sh:105-107` records the cost in registry entries.
- **No completeness assertion on `repo:next-env-drift`'s `deps` list.** That gap is real and
  recorded in CLAUDE.md. It is separate work.
- **The floor proves membership, not semantics.** A key present in `EXPECTED_FINDING_KEYS` whose
  `rows` are always empty satisfies it. The sibling has the same limit.
- **Both pin tables are substring pins.** `ci_targets.py:1669` tests `site not in run_sh_text`;
  `ci/actionlint/run.sh:2084` uses `grep -qF`. `check_self_invocation`'s own docstring
  (`:1646-1649`) records that a commented-out copy satisfies such a test. Adding the two entries
  in section 4 narrows the bypass; it does not make the pins exact.

## 8. Residual risk

Revision 1 claimed one residual. There are more, and the honest list is:

| Residual | Closed by this work? |
|---|---|
| Deleting `run.sh:28`'s flag parse skips the whole negative control | **Yes** (section 4) |
| Deleting `run.sh:714`'s `NEGATIVE` guard | **Yes** (section 4) |
| Commenting out a pinned line satisfies both substring pins | No — inherent to a substring pin |
| `self_test()`'s own `if failures:` report guard (`:3475`) is unpinned | No |

The third and fourth are recorded rather than closed. `ci/actionlint/README.md`'s L16 accepts
the same shape for `ACTIONLINT_SH_CALL_SITES` having no arity floor, and gives the reason: a
floor "would itself need a pin to be honest, which does not terminate the regress".

That reasoning is weaker than revision 1 claimed, and the review was right to press on it: in
this repository the regress **does** terminate, because `repo:actionlint` and
`repo:affected-smoke` are scheduled independently and pin each other. `RUFF_SH_CALL_SITES`
pins three lines inside `ci/ruff/run.sh`'s own self-test for exactly that reason. The fourth
residual above is therefore closable by the same means, and is left open only as scope. It is
noted here so a later reader can pick it up deliberately.

## 9. What changed from revision 1, and why

Revision 1 proposed `main_wiring_findings`, a new pure function that would parse `main()` with
`ast` and report `uncalled`, `unconsumed` and `unreported` names. An adversarial review rejected
it. Three findings held up against the code:

1. **The sibling file already solved this, and better.** `collect_findings` makes two of the
   three findings structurally impossible. Revision 1 proposed a more complex design to *detect*
   what the adjacent file *eliminates*.
2. **Revision 1's replacement was not "strictly stronger".** It derived its name set from live
   module globals, so deleting a whole `check_*` function made it vanish from the set and report
   nothing — while the block revision 1 deleted would have fired. `EXPECTED_FINDING_KEYS`
   restores and generalizes that property.
3. **The reachability argument was overstated.** Revision 1 justified its placement and its
   one-line residual on two pins that do not cover the flag parse those pins depend on.

The AST design is dropped in full, along with its `exempt` parameter and its cardinality,
nested-function and binding-shape questions, all of which are moot once there is no AST walk.

Revision 1's two correct conclusions are kept: section 5's reachability correction, which the
review verified independently, and the refusal to name a function `check_main_wiring` when
`main()` must never call it.

## 10. Files touched

| File | Change |
|---|---|
| `ci/affected-graph/ci_targets.py` | Add `collect_findings` and `EXPECTED_FINDING_KEYS`. Restructure `main()`'s verdict and report onto them. Add the self-test rows. Delete the `main_src.count(...)` block at `:3116-3127`. Add two entries to `RUN_SH_CALL_SITES` and update its fixture. |
| `ci/actionlint/run.sh` | Add two entries to `T_AFFECTED_GRAPH_CALL_SITES`. Update `affected_graph_wiring_self_test`'s fixtures. |
| `ci/next-env/run.sh` | Guard the `dirs` loop on array length. |
| `ci/affected-graph/README.md` | Correct two stale statements. Record the new floor's limits. |
| `CLAUDE.md` | Add an entry for the floor and the two new pin entries. |
| `docs/superpowers/specs/2026-09-17-sma-638-main-wiring-guard-design.md` | This file. |

`ci/affected-graph/ci_targets.py` is linted by `repo:ruff-ci` against `py/pyproject.toml`'s rule
set (`E,F,W,I,N,UP,B,A,C4,SIM,TCH,RUF`, `line-length = 200`). The new code must pass it.

## 11. Verification

| Command | Expected |
|---|---|
| `python3 ci/affected-graph/ci_targets.py --self-test` | rc 0. |
| Delete one key's triple from `collect_findings`, re-run `--self-test` | rc 1, naming the arity and key-sequence failure. |
| Delete `check_tailwind_guard_invocations`' triple, re-run `--self-test` | rc 1. This is the regression SMA-638 reports. |
| Delete `ci/affected-graph/run.sh:28`, re-run `--self-test` | rc 1, naming the missing pin. |
| `bash ci/next-env/run.sh` | rc 0. |
| `moon run repo:affected-smoke --force` | Passes. Needs bash 3.2 **and** Python 3.11 or later — see the PATH note below. |
| `moon run repo:ruff-ci --force` | Passes. Needs bash 4 or later: `export PATH="/opt/homebrew/bin:$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. |
| `repo:actionlint` | **Cannot be verified locally.** CLAUDE.md records that no local bash runs it to completion on this class of machine. CI is the only proof, and section 4 edits that gate. |

**The PATH for `repo:affected-smoke`, measured 2026-09-17.** Moon resolves `bash` through `PATH`,
and Homebrew's bash 5.3.15 deadlocks this gate on this class of machine, so bash 3.2 must win.
But prepending `/bin:/usr/bin` to get it also makes `python3` resolve to `/usr/bin/python3`,
which is **3.9.6**, and `ci/affected-graph/cargo_moon_parity.py` imports `tomllib` — stdlib only
since 3.11. The gate then dies with `ModuleNotFoundError: No module named 'tomllib'` and prints
`negative-control FAILED`, which reads as a gate failure and is not one. Note `ci-targets
self-test OK` prints immediately above it.

Shim the one binary instead, so bash downgrades and nothing else does:

```bash
mkdir -p /tmp/bashshim && ln -sf /bin/bash /tmp/bashshim/bash
export PATH="/tmp/bashshim:$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
moon run repo:affected-smoke --force
```

Measured with that PATH: bash 3.2.57, python3 3.14.7, exit 0 in 13.6s, negative control and real
suite both green. `--force` is required — a cache hit replays a stored log and is not evidence
the gate ran.

Baselines measured before any change, in a worktree provisioned with `proto install`,
`pnpm -C ts install`, `uv sync` and `cargo fetch`: `ci_targets.py --self-test` returned rc 0,
and `bash ci/next-env/run.sh` returned rc 0, reporting both consoles' `next-env.d.ts` as
matching. The gate ran its own `pnpm --dir ... next typegen` call. Whether a `.next` directory
already existed at that point was not recorded, so treat the next-env baseline as evidence that
the gate passes in a provisioned worktree, not as a measurement of a cold-start path.
