# SMA-530 — release-parity negative control in CI — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make all three `repo:release-parity*` Moon tasks run
`ci/release-parity/run.sh --negative-control` before their real run, and guard both the
new invocations and the control block they invoke against silent deletion.

**Architecture:** Three `moon.yml` `script:` blocks (control first, under `set -euo
pipefail`), pinned from `ci/affected-graph/ci_targets.py` — which runs inside the
independently scheduled `repo:affected-smoke`, so no gate judges its own wiring. Two new
pin registries (`moon.yml` lines; `run.sh`'s control block), one new exemption registry
that lets a script-pinned gate opt out of the inputs pin with a recorded reason, and a
self-test fixture **builder** so registering a second gate cannot make the existing
negative assertions vacuous.

**Tech Stack:** Moon 2.3.2 (`moon.yml` `script:` blocks, `moon query tasks`), Bash
(`ci/release-parity/run.sh`, `ci/affected-graph/run.sh`), Python 3 stdlib
(`ci/affected-graph/ci_targets.py` — no test framework; it carries its own `--self-test`).

**Spec:** `docs/superpowers/specs/2026-08-22-sma-530-release-parity-negative-control-design.md`

## Global Constraints

- **Every command needs the proto shims on PATH**, shims FIRST:
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- **Clear the agent-shell env vars when running the parity gate by hand:**
  `env -u AI_AGENT -u CLAUDECODE …`. Otherwise `proto bin` emits NDJSON,
  `RELEASE_PLZ_BIN` resolution breaks (`ecosystems/release-plz.sh:16`) and the gate exits
  rc 2 "INCONCLUSIVE: infrastructure error". This is an agent-environment artifact only.
- **`--negative-control` goes LAST**, after `--ecosystem X`. The pins are byte-exact.
- **`ci_targets.py` has no test framework.** Its tests are fixtures inside `self_test()`,
  run via `python3 ci/affected-graph/ci_targets.py --self-test`. Never add pytest.
- **Never hand-edit `.github/CODEOWNERS`** (Moon-generated).
- **Commit message body must not contain a line starting `word:`** — commitlint parses it
  as a trailer and fails `footer-leading-blank`. Write "owner/repo PR NNN", never `#NNN`.
  Subject must start lowercase and be ≤100 chars.
- **Do not add a new `repo:*` task.** That would require an entry in `ci.yml`'s `T=(…)`
  array AND inside CLAUDE.md's `<!-- ci-targets:begin/end -->` markers. This plan adds none.
- **Never duplicate the `<!-- ci-targets:begin -->` / `<!-- ci-targets:end -->` markers**
  in CLAUDE.md, not even inside backticks — a second copy reds `repo:affected-smoke`.
- **SPDX header** on every new source file (`#` for Python/shell). No new files here.

---

### Task 1: Fixture builder — make `self_test()` survive a second registered gate

Pure refactor. `SELF_SCHEDULED_GATES` still has exactly one key at the end of this task,
and `--self-test` must still pass. This isolates the risky mechanical change from the
feature so a reviewer can reject one without the other.

**Why:** every negative fixture asserts `if not check_self_invocation(...)`, which is
satisfied by **any** missing entry. The moment a second gate is registered, every call
whose `scripts` argument lacks it returns non-empty *regardless of the mutation under
test* — ~24 assertions pass for the wrong reason, and only the positive control at
`:1101` reds, so extending the shared fixture alone looks like a complete fix while
leaving the seven literal-dict fixtures permanently vacuous.

**Files:**
- Modify: `ci/affected-graph/ci_targets.py` (`self_test()`, roughly `:1066-1275`)

**Interfaces:**
- Produces: `wired_scripts(**overrides)` — module-level-adjacent helper defined inside
  `self_test()`, returning `dict[str, str]` keyed by every `SELF_SCHEDULED_GATES` key,
  each value the gate's required lines joined with `\n` and trailing newline. Tasks 2-4
  build every `scripts` fixture through it.

- [ ] **Step 1: Add the builder and a guard that it covers the whole registry**

Replace the current `wired_script` / `scripts` definitions (`ci_targets.py:1066-1074`,
i.e. the `wired_script = (` block through `scripts = {"input-liveness": wired_script}`)
with:

```python
    def wired_scripts(**overrides):
        """A fully-wired `scripts` dict for EVERY SELF_SCHEDULED_GATES key.

        Every negative fixture below asserts `if not check_self_invocation(...)`, which is
        satisfied by ANY missing entry. A literal one-key dict therefore starts passing for
        the WRONG reason the moment a second gate is registered — the exact vacuity this
        gate exists to prevent, and measured on SMA-530: adding three keys turned ~24 of
        these assertions into no-ops while only the positive control red. Building from the
        registry itself means a future gate cannot reopen it; each fixture then mutates
        exactly ONE gate and leaves the rest wired.
        """
        built = {
            task: "".join(f"{line}\n" for line in lines)
            for task, lines in SELF_SCHEDULED_GATES.items()
        }
        built.update(overrides)
        return built

    def broken_script(task, drop):
        """`task`'s wired script with exactly one required line removed."""
        return "".join(
            f"{line}\n" for line in SELF_SCHEDULED_GATES[task] if line != drop
        )

    wired_script = wired_scripts()["input-liveness"]
    scripts = wired_scripts()
    # The builder must not silently under-cover: a typo'd comprehension that dropped a gate
    # would restore the very vacuity it exists to close, and every fixture below would go
    # green together.
    if set(scripts) != set(SELF_SCHEDULED_GATES):
        failures.append(
            f"wired_scripts: covers {sorted(scripts)}, registry has "
            f"{sorted(SELF_SCHEDULED_GATES)}"
        )
```

- [ ] **Step 2: Route every literal one-key `scripts` dict through the builder**

Seven call sites pass a literal dict. Rewrite each so the gate under test is an
*override* and every other gate stays wired. In `ci_targets.py`, replace:

```python
    if not check_self_invocation(wired, {"input-liveness": wired_script.replace(
        "python3 ci/affected-graph/task_inputs.py\n", ""
    )}, wired_actionlint):
        failures.append("check_self_invocation: missed a deleted task_inputs real run (prefix hole)")
    if not check_self_invocation(wired, {"input-liveness": wired_script.replace(
        "python3 ci/affected-graph/task_inputs.py --self-test\n", ""
    )}, wired_actionlint):
        failures.append("check_self_invocation: missed a deleted task_inputs --self-test")
```

with the generic, registry-driven loop that replaces all three named
`input-liveness` deletion fixtures (their SMA-553 provenance is preserved in the comment):

```python
    # SMA-553 D10 + review finding 1, generalised (SMA-530). These three named fixtures used
    # to be spelled out for input-liveness only: the deleted REAL RUN (a strict PREFIX of the
    # --self-test line, so a substring test would report the script fully wired while the gate
    # no longer ran at all), the deleted --self-test, and the deleted `set -euo pipefail`
    # (Moon's script: blocks have no errexit, so deleting it leaves both invocations' TEXT
    # untouched while a failing self-test is silently swallowed — SMA-526). Driving the loop
    # from the registry keeps all three properties asserted for EVERY gate, including ones
    # added later, and covers the same prefix hazard in release-parity*, where the real-run
    # line is likewise a strict prefix of the control line.
    for _task, _lines in sorted(SELF_SCHEDULED_GATES.items()):
        for _line in _lines:
            if not check_self_invocation(
                wired, wired_scripts(**{_task: broken_script(_task, _line)}), wired_actionlint
            ):
                failures.append(
                    f"check_self_invocation: missed {_line!r} deleted from repo:{_task}'s script"
                )
```

Then rewrite the four remaining literal dicts in place:

| current | replacement |
| -- | -- |
| `check_self_invocation(wired, {}, wired_actionlint)` | `check_self_invocation(wired, {}, wired_actionlint)` — **leave as is**; an empty dict is the "absent script" fixture and is correct by construction |
| `check_self_invocation(wired_script, {"input-liveness": wired}, wired_actionlint)` | `check_self_invocation(wired_script, wired_scripts(**{"input-liveness": wired}), wired_actionlint)` |
| `check_self_invocation(no_call, {"input-liveness": wired + wired_script}, wired_actionlint)` | `check_self_invocation(no_call, wired_scripts(**{"input-liveness": wired + wired_script}), wired_actionlint)` |
| `check_self_invocation(wired, {"input-liveness": wired_script + wired_actionlint}, no_actionlint_call)` | `check_self_invocation(wired, wired_scripts(**{"input-liveness": wired_script + wired_actionlint}), no_actionlint_call)` |

- [ ] **Step 3: Add an indented-but-wired positive fixture**

Moon resolves a YAML block scalar to column-0 lines, but `check_self_invocation` strips
both sides for task scripts (`:673`) and that tolerance is currently unasserted:

```python
    # The task-script haystack strips BOTH sides (:673) — unlike the actionlint haystack's
    # column-0 rule — because Moon task scripts are indented inside YAML. Assert that
    # tolerance directly: a wired-but-indented script must NOT be reported missing.
    indented_task_script = "".join(
        f"  {line}\n" for line in SELF_SCHEDULED_GATES["input-liveness"]
    )
    if check_self_invocation(
        wired, wired_scripts(**{"input-liveness": indented_task_script}), wired_actionlint
    ):
        failures.append("check_self_invocation: an indented but fully wired script was reported missing")
```

- [ ] **Step 4: Run the self-test — must still pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py --self-test; echo "rc=$?"
```

Expected: `rc=0`. Nothing about the gate's behaviour changed — only how fixtures are built.

- [ ] **Step 5: Prove the loop actually fires (temporary mutation)**

Temporarily break `check_self_invocation` by making the task-script half a no-op — change
`:672-674`'s loop body to `continue` — then re-run. Expected: `rc=1`, with one
`missed '…' deleted from repo:input-liveness's script` row **per required line** (3 rows).
Revert the mutation and confirm `rc=0` again.

- [ ] **Step 6: Run the gate end-to-end**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/affected-graph/run.sh --negative-control && ci/affected-graph/run.sh; echo "rc=$?"
```

Expected: `rc=0` for both.

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/ci_targets.py
git commit -m "refactor(repo): build ci_targets self-test script fixtures from the registry (SMA-530)"
```

---

### Task 2: Wire the three tasks and pin the nine lines

TDD order is deliberate: the pin lands FIRST and must red against the real, unwired
`moon.yml`. That red **is** the failing test — it proves the pin is reachable and bites.
Do not skip ahead to `moon.yml`.

**Files:**
- Modify: `ci/affected-graph/ci_targets.py` (`SELF_SCHEDULED_GATES` at `:217`; new
  `SELF_TASK_GLOBS_EXEMPT`; new `check_registry_pairing`; the pairing assert at `:1295`)
- Modify: `moon.yml` (`release-parity` `:57-65`, `release-parity-py` `:67-76`,
  `release-parity-ts` `:78-89`)

**Interfaces:**
- Consumes: `wired_scripts` / `broken_script` from Task 1.
- Produces: `check_registry_pairing(scheduled, globs, exempt)` →
  `(unpinned, bad_exempt, stale_exempt, both, orphan_globs)`, five sorted lists of task
  names. Task 4's `main()` wiring and Task 5's docs both reference it.

- [ ] **Step 1: Add the three registry entries**

In `ci/affected-graph/ci_targets.py`, extend `SELF_SCHEDULED_GATES` (`:217-223`):

```python
SELF_SCHEDULED_GATES = {
    "input-liveness": (
        "set -euo pipefail",
        "python3 ci/affected-graph/task_inputs.py --self-test",
        "python3 ci/affected-graph/task_inputs.py",
    ),
    # SMA-530. Three sibling tasks over one script, each with its own control: their inputs
    # are DISJOINT (moon.yml:61-89), so a PR touching only ts/packages/paigasus-sdk/
    # .releaserc.json selects release-parity-ts and neither sibling — one shared control
    # would leave that PR running a parity gate with nothing proving it can report red.
    # Measured net cost +890ms/+733ms/+1111ms per task (~20%).
    #
    # WHOLE-LINE matched, and that is load-bearing in one direction here: the real-run line
    # is a strict PREFIX of the control line in all three tasks, so a substring test would
    # let the REAL RUN be deleted while this pin stayed green. `set -euo pipefail` is pinned
    # as a first-class required line for the reason recorded at :199-209 — Moon's script:
    # blocks have no errexit, so deleting it leaves both invocations' text untouched while a
    # failing control is silently swallowed.
    #
    # These pin the moon.yml INVOCATION only. The control BLOCK they invoke
    # (ci/release-parity/run.sh:60-69) is pinned separately by RELEASE_PARITY_SH_CALL_SITES
    # below — deleting the block while leaving the flag parse makes --negative-control fall
    # through to the real suite and exit 0, which these entries cannot see.
    "release-parity": (
        "set -euo pipefail",
        "ci/release-parity/run.sh --negative-control",
        "ci/release-parity/run.sh",
    ),
    "release-parity-py": (
        "set -euo pipefail",
        "ci/release-parity/run.sh --ecosystem python-semantic-release --negative-control",
        "ci/release-parity/run.sh --ecosystem python-semantic-release",
    ),
    "release-parity-ts": (
        "set -euo pipefail",
        "ci/release-parity/run.sh --ecosystem semantic-release --negative-control",
        "ci/release-parity/run.sh --ecosystem semantic-release",
    ),
}

# SMA-530. A script-pinned gate whose `inputs` are NOT separately pinned must say so here,
# with a reason — the repo's established idiom (T_EXEMPT, ALLOW_DEAD_INPUT,
# ALLOW_NO_CARGO_BACKING, BRANCH_SKIP, COE_SKIP all work this way).
#
# Why an exemption rather than dropping the pairing rule: repo:affected-smoke's own inputs
# are the most load-bearing input list in the repo (moon.yml:130-162, several entries
# carrying explicit do-not-remove comments), so when it is script-pinned later it MUST also
# have its globs pinned. A plain subset rule would let that be skipped in silence.
SELF_TASK_GLOBS_EXEMPT = {
    "release-parity": (
        "narrow ecosystem-specific globs, unlike input-liveness's `**/*` which IS the thing "
        "that gate exists to protect; declared-glob liveness is asserted generically by "
        "repo:input-liveness (ci/affected-graph/task_inputs.py), so a second exact-match copy "
        "here would red on every legitimate inputs edit and buy nothing"
    ),
    "release-parity-py": "as release-parity",
    "release-parity-ts": "as release-parity",
}
```

- [ ] **Step 2: Replace the pairing assert with a testable verdict function**

The current check is inline inside `self_test()` (`:1294-1299`) and so cannot be driven
with fixtures. Extract it. Add next to `check_gate_inputs` (after it, around `:686`):

```python
def check_registry_pairing(scheduled=None, globs=None, exempt=None):
    """SMA-530. The three self-scheduled-gate registries must stay consistent.

    Returns (unpinned, bad_exempt, stale_exempt, both, orphan_globs), all sorted name lists.

    Replaces a bare `set(A) != set(B)` equality. Equality forced every script-pinned gate to
    duplicate its input globs here; a plain subset would have let repo:affected-smoke be
    script-pinned later WITHOUT pinning the inputs that make every pin in this file
    reachable. An exemption with a recorded reason keeps the decision explicit and visible.
    """
    scheduled = SELF_SCHEDULED_GATES if scheduled is None else scheduled
    globs = SELF_TASK_EXPECTED_GLOBS if globs is None else globs
    exempt = SELF_TASK_GLOBS_EXEMPT if exempt is None else exempt
    return (
        sorted(t for t in scheduled if t not in globs and t not in exempt),
        sorted(t for t, reason in exempt.items() if not (reason or "").strip()),
        sorted(set(exempt) - set(scheduled)),
        sorted(set(globs) & set(exempt)),
        sorted(set(globs) - set(scheduled)),
    )
```

Then replace the inline assert at `:1294-1299` with fixture-driven coverage:

```python
    # SMA-530 — the three registries, driven with fixtures rather than asserted inline, so
    # each row can be shown to fire.
    def pairing(label, scheduled, globs, exempt, want):
        got = check_registry_pairing(scheduled, globs, exempt)
        if got != want:
            failures.append(f"check_registry_pairing[{label}]: got {got}, want {want}")

    pairing("real-registries", None, None, None, ([], [], [], [], []))
    pairing("unpinned", {"g": ()}, {}, {}, (["g"], [], [], [], []))
    pairing("pinned-by-globs", {"g": ()}, {"g": ("**/*",)}, {}, ([], [], [], [], []))
    pairing("pinned-by-exemption", {"g": ()}, {}, {"g": "reason"}, ([], [], [], [], []))
    pairing("empty-reason", {"g": ()}, {}, {"g": "   "}, ([], ["g"], [], [], []))
    pairing("stale-exemption", {}, {}, {"ghost": "outlived its task"}, ([], [], ["ghost"], [], []))
    pairing("exempt-and-pinned", {"g": ()}, {"g": ("**/*",)}, {"g": "r"}, ([], [], [], ["g"], []))
    pairing("orphan-globs", {}, {"ghost": ("**/*",)}, {}, ([], [], [], [], ["ghost"]))
```

Note `pairing("real-registries", None, None, None, ...)` is what keeps the LIVE constants
honest — it is the replacement for the deleted inline assert.

- [ ] **Step 3: Run the self-test and the gate — expect the pin to RED**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py --self-test; echo "self-test rc=$?"
python3 ci/affected-graph/ci_targets.py; echo "gate rc=$?"
```

Expected: **self-test `rc=0`** (fixtures are self-contained), **gate `rc=1`** listing nine
missing rows of the form `release-parity script: set -euo pipefail`,
`release-parity script: ci/release-parity/run.sh --negative-control`, … one per pinned
line per task. This is the failing test. If the gate passes here, the pin is not wired —
stop and diagnose before continuing.

- [ ] **Step 4: Wire the three Moon tasks**

In `moon.yml`, replace the three one-line `script:` values. `release-parity` (`:57-65`):

```yaml
  release-parity:
    description: 'Dry-run release-plz over synthetic commits; assert commit->semver parity (SMA-398).'
    # Negative control FIRST, mirroring repo:publish-metadata, repo:affected-smoke and
    # repo:input-liveness (SMA-530). Without it CI runs only the real suite, and the real
    # suite CANNOT detect a check_case that has lost the ability to report red — that is
    # precisely when it is green. Measured: changing run.sh:51 to drop the `got_a` half of
    # the comparison leaves all five cases.tsv rows passing (slot b is at baseline in every
    # one) and the gate exits 0 while asserting nothing; only the control catches it.
    #
    # Moon does not enable errexit for `script:` blocks — the block's status is its LAST
    # command's — so `set -euo pipefail` is REQUIRED, or a failing control is masked by the
    # passing real run. run.sh's own `set -euo pipefail` governs its body, not this block.
    # Same trap repo:promtool, repo:nats-permissions and repo:publish-metadata document.
    #
    # All three sibling tasks carry their own control because their inputs are DISJOINT: a
    # PR touching only ts/packages/paigasus-sdk/.releaserc.json selects release-parity-ts
    # alone. Net cost measured at +890ms/+733ms/+1111ms (~20% each). These three lines are
    # pinned by SELF_SCHEDULED_GATES in ci/affected-graph/ci_targets.py — edit them and that
    # gate reds until the constant agrees.
    script: |
      set -euo pipefail
      ci/release-parity/run.sh --negative-control
      ci/release-parity/run.sh
    toolchain: 'system'
    inputs:
      - 'ci/release-parity/**/*'
      - 'rs/release-plz.toml'
      - '.prototools'
      - '.proto/plugins/release-plz.toml'
```

`release-parity-py` (`:67-76`) — same shape, `--ecosystem python-semantic-release` on
**both** lines, control last:

```yaml
    # Negative control FIRST — see repo:release-parity above for the full reasoning; this
    # task carries its own because the three tasks' inputs are disjoint (SMA-530).
    script: |
      set -euo pipefail
      ci/release-parity/run.sh --ecosystem python-semantic-release --negative-control
      ci/release-parity/run.sh --ecosystem python-semantic-release
```

`release-parity-ts` (`:78-89`):

```yaml
    # Negative control FIRST — see repo:release-parity above for the full reasoning; this
    # task carries its own because the three tasks' inputs are disjoint (SMA-530).
    script: |
      set -euo pipefail
      ci/release-parity/run.sh --ecosystem semantic-release --negative-control
      ci/release-parity/run.sh --ecosystem semantic-release
```

Leave each task's `description`, `toolchain` and `inputs` untouched.

- [ ] **Step 5: Run the gate — now GREEN**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py; echo "gate rc=$?"
```

Expected: `rc=0`. If a row survives, the `moon.yml` text and the constant disagree
byte-for-byte — compare them character by character (flag order, `--ecosystem` value).

- [ ] **Step 6: Confirm the control actually runs under Moon**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
env -u AI_AGENT -u CLAUDECODE moon run repo:release-parity --force 2>&1 | tail -20
```

Expected: `== negative control: feeding a deliberately wrong expectation ==`, then
`negative-control OK: harness reported red as expected`, then the five `PASS` rows and
`== all parity cases passed ==`. Repeat for `repo:release-parity-py` and
`repo:release-parity-ts`.

- [ ] **Step 7: Commit**

```bash
git add moon.yml ci/affected-graph/ci_targets.py
git commit -m "feat(repo): run release-parity's negative control in CI on all three tasks (SMA-530)"
```

---

### Task 3: Pin the control block itself, and make the pin reachable

Closes the half Task 2 cannot see: `run.sh:14` parses `--negative-control` into
`NEGATIVE`, and `:60-69` is the block that acts on it. Delete the block but keep the flag
parse and `run.sh --negative-control` falls through to the real suite, exits 0, all three
tasks stay green, and CI silently runs the five-case suite twice per task.

**Files:**
- Modify: `ci/affected-graph/ci_targets.py` (new `RELEASE_PARITY_SH_CALL_SITES`;
  `check_self_invocation` at `:655`; `main()` at `:1384`; fixtures)
- Modify: `moon.yml` (`repo:affected-smoke` `inputs`, `:130-162`)

**Interfaces:**
- Consumes: `wired_scripts` (Task 1).
- Produces: `check_self_invocation(run_sh_text, scripts, actionlint_sh_text,
  release_parity_sh_text)` — a **fourth REQUIRED positional** parameter.

- [ ] **Step 1: Add the registry**

After `ACTIONLINT_SH_CALL_SITES` in `ci/affected-graph/ci_targets.py`:

```python
# SMA-530. The moon.yml pins above prove the CONTROL IS INVOKED; these prove it still DOES
# something. run.sh:14 parses --negative-control into NEGATIVE and :60-69 is the block that
# acts on it — delete the block while leaving the flag parse and `run.sh --negative-control`
# falls straight through to the real suite, exits 0, and all three tasks stay green while CI
# runs the five-case suite twice per task. SELF_SCHEDULED_GATES cannot see that: it pins
# moon.yml text, not semantics. Same class as ACTIONLINT_SH_CALL_SITES above, and the same
# lesson SMA-542 I1 and CodeRabbit round 4 C1 each cost a round to learn — a gate check's own
# call site is what goes unguarded.
#
# REACHABILITY IS NOT AUTOMATIC. This check only runs when repo:affected-smoke is scheduled,
# so moon.yml lists `ci/release-parity/**/*` among its inputs. Without that entry the PR
# deleting this block is exactly the PR that does not schedule this gate. Do not remove it.
#
# Matched as stripped WHOLE LINES, not substrings: the two `echo` lines are the case arms
# that give the control its verdict AND its exit status, and a substring match on the message
# text alone would survive `exit 0`/`exit 1` being swapped or dropped.
RELEASE_PARITY_SH_CALL_SITES = (
    'if [ "$NEGATIVE" = 1 ]; then',
    '1) echo "negative-control OK: harness reported red as expected"; exit 0 ;;',
    '0) echo "negative-control FAILED: harness accepted a wrong expectation" >&2; exit 1 ;;',
)
```

- [ ] **Step 2: Extend `check_self_invocation` with a fourth REQUIRED haystack**

In `ci_targets.py:655`, change the signature and add the check. The parameter is
positional and required — the docstring already records why an optional one defaulting to
`""` re-creates the hole it exists to close:

```python
def check_self_invocation(run_sh_text, scripts, actionlint_sh_text, release_parity_sh_text):
```

and before `return missing`, add:

```python
    # Stripped whole lines, like the task-script haystack and unlike the column-0 actionlint
    # one: these three sit inside run.sh at varying indentation (the `case` arms are indented
    # four spaces), so a column-0 rule would reject the real, executing lines.
    release_parity_lines = {line.strip() for line in release_parity_sh_text.splitlines()}
    missing.extend(
        f"ci/release-parity/run.sh: {site}"
        for site in RELEASE_PARITY_SH_CALL_SITES
        if site not in release_parity_lines
    )
```

Update the docstring's "The three texts are checked SEPARATELY" to "The four texts".

- [ ] **Step 3: Wire it in `main()`**

Two edits. First, inside the `try:` block, immediately after the `actionlint_sh`
read at `ci_targets.py:1361-1363`, add a fourth read in the same form (it must stay INSIDE
the try, so an unreadable file routes to rc 2 via `INFRA_ERRORS` rather than raising):

```python
        release_parity_sh = read_input(
            root / "ci" / "release-parity" / "run.sh", "ci/release-parity/run.sh"
        )
```

Second, at `:1384`, pass it as the fourth argument:

```python
    missing_sites = check_self_invocation(run_sh, scripts, actionlint_sh, release_parity_sh)
```

Extend the `missing_sites` diagnostic at `:1440-1449` so the new prefix is explained:

```python
         "    A row prefixed `ci/release-parity/run.sh:` means the --negative-control FLAG is\n"
         "    still parsed but the block that acts on it is gone, so the control silently runs\n"
         "    the real suite twice and can no longer report red."),
```

- [ ] **Step 4: Update every existing `check_self_invocation` fixture call**

All ~25 calls in `self_test()` now need a fourth argument. Add the wired fixture beside
`wired_actionlint`:

```python
    wired_release_parity = (
        'if [ "$NEGATIVE" = 1 ]; then\n'
        '  echo "== negative control ... =="\n'
        '  case "$ec" in\n'
        '    1) echo "negative-control OK: harness reported red as expected"; exit 0 ;;\n'
        '    0) echo "negative-control FAILED: harness accepted a wrong expectation" >&2; exit 1 ;;\n'
        '  esac\n'
        'fi\n'
    )
```

Pass `wired_release_parity` as the fourth argument to every existing call, then add the
deletion fixtures — one per pinned line, driven from the registry so a fourth entry is
covered automatically:

```python
    # SMA-530 — one row per pinned line, so a mutant that widened the match back to
    # "matches anywhere" is caught regardless of which entry it is tested against.
    for _site in RELEASE_PARITY_SH_CALL_SITES:
        _broken = "".join(
            line for line in wired_release_parity.splitlines(keepends=True)
            if line.strip() != _site
        )
        if not check_self_invocation(wired, scripts, wired_actionlint, _broken):
            failures.append(
                f"check_self_invocation: missed {_site!r} deleted from ci/release-parity/run.sh"
            )
    # Contamination: a release-parity site must not be satisfiable from another haystack.
    if not check_self_invocation(
        wired + wired_release_parity, scripts, wired_actionlint,
        "".join(line for line in wired_release_parity.splitlines(keepends=True)
                if line.strip() != RELEASE_PARITY_SH_CALL_SITES[0])
    ):
        failures.append(
            "check_self_invocation: a release-parity site was satisfied by run.sh text"
        )
```

- [ ] **Step 5: Add the reachability input**

In `moon.yml`, inside `repo:affected-smoke`'s `inputs` (`:130-162`), after the
`ci/actionlint/**/*` entry:

```yaml
      # SMA-530 — this task now pins ci/release-parity/run.sh's --negative-control BLOCK
      # (RELEASE_PARITY_SH_CALL_SITES), so a change under ci/release-parity/ MUST re-key it.
      # Without this the pin is real but unreachable: the PR that deletes the block does not
      # schedule this task. Same reasoning as the ci/actionlint/**/* entry above.
      - 'ci/release-parity/**/*'
```

- [ ] **Step 6: Verify the pin bites, and is reachable**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py --self-test; echo "self-test rc=$?"   # expect 0
python3 ci/affected-graph/ci_targets.py; echo "gate rc=$?"                    # expect 0

# The hole, demonstrated: delete the block, keep the flag parse.
cp ci/release-parity/run.sh /tmp/rp.bak
python3 - <<'PY'
import re
p = "ci/release-parity/run.sh"
s = open(p).read()
start = s.index('if [ "$NEGATIVE" = 1 ]; then')
end = s.index("\nfi\n", start) + len("\nfi\n")
open(p, "w").write(s[:start] + s[end:])
PY
env -u AI_AGENT -u CLAUDECODE bash ci/release-parity/run.sh --negative-control >/dev/null 2>&1
echo "gutted control rc=$?"                                   # expect 0 — THE HOLE
python3 ci/affected-graph/ci_targets.py; echo "gate rc=$?"     # expect 1, three rows
cp /tmp/rp.bak ci/release-parity/run.sh && touch ci/release-parity/run.sh
python3 ci/affected-graph/ci_targets.py; echo "restored rc=$?" # expect 0
```

Then confirm reachability — a `ci/release-parity/**` edit must now select the gate:

```bash
printf '\n# probe\n' >> ci/release-parity/README.md
env -u AI_AGENT -u CLAUDECODE moon query tasks --affected 2>/dev/null \
  | python3 -c "import json,sys; d=json.load(sys.stdin); [print(f'{p}:{t}') for p,ts in d['tasks'].items() for t in ts]" | sort
git checkout ci/release-parity/README.md
```

Expected: the list includes `repo:affected-smoke` **and** all three `repo:release-parity*`.

> **`touch` after restoring is not optional.** Restoring a file from a backup rolls its
> mtime backwards, and tools that cache on mtime then replay the result from the mutated
> version — a documented time sink in this repo.

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/ci_targets.py moon.yml
git commit -m "feat(repo): pin release-parity's negative-control block against deletion (SMA-530)"
```

---

### Task 4: Pin the three tasks as CI-eligible

`check_forward` computes `want = eligible - exempt` and `got = T ∩ repo` (`:513-533`), so
dropping all three from `T` **and** flipping them CI-ineligible shrinks both sets
consistently and passes green. A control whose survival depends on nobody switching the
task off is the same shape as one that runs only when a human remembers.

**Files:**
- Modify: `ci/affected-graph/ci_targets.py` (`REQUIRED_REPO_TASKS` at `:151`;
  `check_floor` fixture at `:917-921`)

**Interfaces:**
- Consumes: nothing new.
- Produces: nothing new.

- [ ] **Step 1: Extend the floor**

```python
# The floor. C1 compares two derived sets, and two EMPTY sets compare equal — so a project-id
# filter that stops matching, or a moon output shape change, would print PASS while asserting
# nothing. Every task named here must be present and CI-eligible in the parsed `repo` set.
# Same role as cargo_moon_parity.py's REQUIRED_FFI_TASKS.
#
# The three release-parity* tasks joined the floor with SMA-530: they now carry a negative
# control, and check_forward's `want`/`got` shrink CONSISTENTLY when a task is dropped from
# `T` and made CI-ineligible in the same edit — so without a floor entry the control could be
# switched off entirely with every check green.
REQUIRED_REPO_TASKS = (
    "affected-smoke",
    "promtool",
    "publish-metadata",
    "release-parity",
    "release-parity-py",
    "release-parity-ts",
)
```

- [ ] **Step 2: Update the `check_floor` fixture**

At `ci_targets.py:919-921` the fixture asserts the exact missing list. Replace:

```python
    thin = {"repo": {"deny": True}}
    if check_floor(thin) != ["affected-smoke", "promtool", "publish-metadata"]:
        failures.append(f"check_floor: did not name every absent floor member: {check_floor(thin)}")
```

with a registry-driven form, so a future floor member cannot be added without coverage:

```python
    thin = {"repo": {"deny": True}}
    if check_floor(thin) != sorted(REQUIRED_REPO_TASKS):
        failures.append(f"check_floor: did not name every absent floor member: {check_floor(thin)}")
```

`tasks_fixture` (`:869-873`) and `aligned_t` (`:874`) must BOTH gain the three names, or
this task reds the fixtures it did not touch. `tasks_fixture` feeds
`check_floor(tasks_fixture) != []` just above, and `aligned_t` feeds every `forward(...)`
case — `check_forward` computes `want = eligible - exempt`, so three newly-eligible tasks
absent from `aligned_t` would surface as `missing` in each. Replace both:

```python
    tasks_fixture = {
        "repo": {"deny": True, "promtool": True, "affected-smoke": True,
                 "publish-metadata": True, "install-hooks": False,
                 # SMA-530 — floor members, so they must be CI-eligible here too.
                 "release-parity": True, "release-parity-py": True,
                 "release-parity-ts": True},
        "some-crate-rs": {"build": True, "test": True, "build-release": True},
    }
    aligned_t = ["build", "test", "deny", "promtool", "affected-smoke", "publish-metadata",
                 "release-parity", "release-parity-py", "release-parity-ts"]
```

Read both lines before editing to confirm they still match this shape.

- [ ] **Step 3: Verify**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py --self-test; echo "self-test rc=$?"   # expect 0
python3 ci/affected-graph/ci_targets.py; echo "gate rc=$?"                    # expect 0
```

Then prove the floor bites: temporarily add `runInCI: false` to `repo:release-parity-ts`
in `moon.yml`, re-run the gate, expect `rc=1` naming `release-parity-ts` as an absent
floor member. Remove it and confirm `rc=0`.

- [ ] **Step 4: Commit**

```bash
git add ci/affected-graph/ci_targets.py
git commit -m "feat(repo): pin the release-parity tasks as CI-eligible floor members (SMA-530)"
```

---

### Task 5: Documentation

Also serves a mechanical purpose: `ci/release-parity/**/*` is an input to all three tasks
(`moon.yml:62, 71, 83`), so the README edit re-keys them and CI runs the new control on
the PR that introduces it. A `moon.yml`-only edit does **not** select these tasks
(measured — it selects `repo:actionlint`, `repo:affected-smoke`, `repo:input-liveness`
only), so without this the change would ship unexecuted.

**Files:**
- Modify: `ci/release-parity/README.md` (append a section)
- Modify: `ci/affected-graph/README.md` (`:124` and the maintenance paragraph `:137-149`)
- Modify: `CLAUDE.md` (one Gotchas bullet)

- [ ] **Step 1: `ci/release-parity/README.md` — append**

````markdown
## The negative control runs in CI (SMA-530)

All three Moon tasks run `--negative-control` before the real suite, under an explicit
`set -euo pipefail`:

```yaml
script: |
  set -euo pipefail
  ci/release-parity/run.sh --negative-control
  ci/release-parity/run.sh
```

**Why the real run cannot substitute for it.** Change `run.sh:51` from
`if [ "$got_a" = "$expected" ] && [ "$got_b" = "$BASELINE" ]` to
`if [ "$got_b" = "$BASELINE" ]`. Slot `b` is at baseline in all five `cases.tsv` rows, so
the real run prints `== all parity cases passed ==` and exits 0 — the gate is vacuous. The
control reds. A gate that has lost the ability to report red is green exactly when it
matters.

**Why the pipefail line.** Moon does not enable errexit for `script:` blocks, so the
block's status is its LAST command's: without it a failing control is masked by the
passing real run. `run.sh`'s own `set -euo pipefail` governs its body, not the Moon block.

**Why all three tasks.** Their inputs are disjoint (`moon.yml:61-89`), so a PR touching
only `ts/packages/paigasus-sdk/.releaserc.json` selects `release-parity-ts` and neither
sibling; one shared control would leave that PR uncontrolled. Net measured cost
+890ms/+733ms/+1111ms (~20% each). Note the control's per-ecosystem code path is a strict
*subset* of the real run's — the argument is affectedness and symmetry, not extra
adapter coverage.

**What guards it.** `ci/affected-graph/ci_targets.py` pins the nine `moon.yml` lines
(`SELF_SCHEDULED_GATES`) and the control block itself
(`RELEASE_PARITY_SH_CALL_SITES`), from inside `repo:affected-smoke` — a separately
scheduled gate, so neither judges its own wiring. `repo:affected-smoke` lists
`ci/release-parity/**/*` in its inputs to make the second pin reachable.

### Limitations

- **L1 — `repo:affected-smoke`'s own `moon.yml` input is unpinned.** Every pin above
  depends on `- 'moon.yml'` (`moon.yml:134`). Deleting that entry is itself a root
  `moon.yml` edit, and afterwards the task's remaining globs do not match the root file
  (`*/moon.yml` matches `rs/moon.yml`, not `moon.yml`; `.moon/**/*` does not match it), so
  the removal PR would not schedule the gate. Pre-existing — the `input-liveness` pin
  rests on the same entry — and closing it needs a *containment* variant of
  `SELF_TASK_EXPECTED_GLOBS`, which is today an exact match.
- **L2 — the task-script haystack strips both sides** (`ci_targets.py:673`), so an
  indented copy inside `if false; then … fi` satisfies the pin. The column-0 rule that
  rejects this for the actionlint haystack is unavailable here: Moon task scripts are
  indented inside YAML. Separately, `set +e` inserted *after* the pipefail line satisfies
  all three pins while re-opening the masking they exist to prevent.
- **L3 — whole-line pins are brittle in the false-red direction.** Making the base task's
  ecosystem explicit (`--ecosystem release-plz`), adding a trailing comment, or reordering
  flags reds the gate although nothing is broken. Restore the exact line or update the
  constant.
- **L4 — the control's `0.1.1` is coupled to `cases.tsv`'s contract.** `run.sh:62-63`
  hardcodes it as "deliberately wrong" for `fix!` in 0.x. Should the canonical contract
  ever change so that value becomes correct, all three controls red spuriously and the
  diagnosis is non-obvious.
````

- [ ] **Step 2: `ci/affected-graph/README.md` — correct the stale C4 description**

At `:124` the text reads "(`SELF_SCHEDULED_GATES`, whole-line-matched — currently
`repo:input-liveness`'s, SMA-553)". Replace "currently `repo:input-liveness`'s, SMA-553"
with "`repo:input-liveness`'s and the three `repo:release-parity*`, SMA-553 / SMA-530",
and add a fourth haystack to the sentence listing them:
"and `ci/release-parity/run.sh`'s own `--negative-control` block
(`RELEASE_PARITY_SH_CALL_SITES`, whole-line-matched — SMA-530)".

In the maintenance paragraph (`:137-149`), add:

```markdown
  A script-pinned gate must also have its `inputs` pinned (`SELF_TASK_EXPECTED_GLOBS`) or
  carry a reasoned `SELF_TASK_GLOBS_EXEMPT` entry; an exemption naming no script-pinned
  gate, or one with a blank reason, is itself reported. The registries were equality-paired
  until SMA-530 — a plain subset would have let `repo:affected-smoke` be script-pinned
  later without pinning the inputs that make every pin in this file reachable.
```

- [ ] **Step 3: `CLAUDE.md` — one Gotchas bullet**

Add after the existing `repo:actionlint`/`repo:affected-smoke` guard-each-other bullet.
**Do not touch the `<!-- ci-targets:begin/end -->` block or duplicate its markers.**

```markdown
- All three `repo:release-parity*` tasks run `ci/release-parity/run.sh --negative-control`
  before their real run, under an explicit `set -euo pipefail` (SMA-530). Each carries its
  own control because their `inputs` are disjoint — a PR touching only a `.releaserc.json`
  selects `-ts` alone. Two pins guard it, both living in `ci/affected-graph/ci_targets.py`
  and both running inside `repo:affected-smoke`: `SELF_SCHEDULED_GATES` pins the nine
  `moon.yml` lines (byte-exact whole lines — reordering a flag or adding a trailing comment
  reds it), and `RELEASE_PARITY_SH_CALL_SITES` pins the control BLOCK, because deleting
  `run.sh:60-69` while leaving the flag parse makes `--negative-control` fall through to the
  real suite and exit 0. That second pin is reachable only because `repo:affected-smoke`
  lists `ci/release-parity/**/*` in its `inputs` — do not remove it. A script-pinned gate
  needs either a `SELF_TASK_EXPECTED_GLOBS` entry or a reasoned `SELF_TASK_GLOBS_EXEMPT`
  one. Note a `moon.yml`-only edit does NOT select the `release-parity*` tasks (their own
  `script:` is not among their inputs), so a PR changing those blocks should also touch
  `ci/release-parity/**` if it wants CI to execute them.
```

- [ ] **Step 4: Verify docs did not break a gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py; echo "gate rc=$?"      # CLAUDE.md is parsed by C3
grep -c "ci-targets:begin" CLAUDE.md                            # MUST be exactly 1
grep -c "ci-targets:end" CLAUDE.md                              # MUST be exactly 1
```

- [ ] **Step 5: Commit**

```bash
git add ci/release-parity/README.md ci/affected-graph/README.md CLAUDE.md
git commit -m "docs(repo): record the release-parity negative-control contract and its pins (SMA-530)"
```

---

### Task 6: Whole-branch verification

No new code unless a check fails. This is the spec's Verification section executed end to
end; fix anything it surfaces and re-run.

**Files:**
- Modify: whatever the checks surface (expected: none)

- [ ] **Step 1: The premise mutation, per ecosystem**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cp ci/release-parity/run.sh /tmp/rp2.bak
python3 - <<'PY'
p = "ci/release-parity/run.sh"
s = open(p).read()
old = '  if [ "$got_a" = "$expected" ] && [ "$got_b" = "$BASELINE" ]; then'
assert s.count(old) == 1, "anchor not unique — read the file and re-anchor"
open(p, "w").write(s.replace(old, '  if [ "$got_b" = "$BASELINE" ]; then'))
PY
for t in release-parity release-parity-py release-parity-ts; do
  out=$(env -u AI_AGENT -u CLAUDECODE moon run repo:$t --force 2>&1); rc=$?
  echo "$t rc=$rc  all-passed-present=$(printf '%s' "$out" | grep -c 'all parity cases passed')"
done
cp /tmp/rp2.bak ci/release-parity/run.sh && touch ci/release-parity/run.sh
```

Expected for each: `rc` non-zero and `all-passed-present=0` — the control stopped the task
before the real run. (`touch` after restore is required; a restored mtime makes cached
tooling replay the mutated result.)

- [ ] **Step 2: Clean tree**

```bash
for t in release-parity release-parity-py release-parity-ts; do
  env -u AI_AGENT -u CLAUDECODE moon run repo:$t --force >/dev/null 2>&1; echo "$t rc=$?"
done
```

Expected: `rc=0` for all three.

- [ ] **Step 3: `set -euo pipefail` is load-bearing (demonstration)**

Temporarily delete the `set -euo pipefail` line from `repo:release-parity`'s script AND
apply Step 1's mutation. Run `moon run repo:release-parity --force`. Expected: the task
reports **success** despite the failing control — the exact masking D3 describes. Restore
both.

- [ ] **Step 4: The `moon.yml` pin bites, all nine lines**

For each of the nine pinned lines, delete it from `moon.yml`, run
`python3 ci/affected-graph/ci_targets.py`, confirm `rc=1` and that the output names that
exact line, then restore. Script it rather than doing it by hand.

- [ ] **Step 5: Full CI graph**

Run the command between CLAUDE.md's `<!-- ci-targets:begin -->` / `<!-- ci-targets:end -->`
markers verbatim, with the proto shims on PATH. If Moon reports an unattributed failure,
diagnose via `jq '.actions[]|select(.status=="failed")' .moon/cache/ciReport.json`.

Pay particular attention to `repo:affected-smoke` — adding `ci/release-parity/**/*` to its
inputs may shift an `assert_task_case` baseline in `ci/affected-graph/run.sh`. If so,
re-baseline it and say so in the commit message.

- [ ] **Step 6: Commit any fixes**

```bash
git add -A
git commit -m "test(repo): re-baseline the affected-graph task cases for the new input (SMA-530)"
```

Skip this step entirely if nothing needed fixing.
