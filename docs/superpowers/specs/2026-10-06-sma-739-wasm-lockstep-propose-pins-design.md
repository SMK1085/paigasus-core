# SMA-739: pin the propose checker steps of wasm-lockstep

- Linear: SMA-739
- Date: 2026-10-06
- Base: `main` at `15bd61b7` (SMA-738, PR 390, merged)
- Related: SMA-738 spec `docs/superpowers/specs/2026-10-05-sma-738-wasm-lockstep-edge-flips-design.md`, section 4.5

## 1. Problem

The `propose` job of `.github/workflows/wasm-lockstep.yml` has two steps that end with a
`lockstep_check.py` command:

| Step | Last command | What the command proves |
|---|---|---|
| `verify` | `lockstep_check.py artifact …` | AC4.1 and AC4.2: the downloaded artifact and the lock are acceptable. |
| `apply` | `lockstep_check.py status --file …` | The copy changed only the allowed files. |

`pin_check.py` P18 checks only that the checker command is the last whole command of its step,
with nothing joined to it. A command BEFORE the checker command is not checked. These four
forms before the checker command pass `pin_check.py` today:

- `exit 0`
- a bare `exit`
- `set -n`
- `set -o noexec`

With each form, the checker does not run and the step exits 0. Then the token step mints the App
token, and the job pushes a branch that no checker accepted. The final review of SMA-738 found the
`verify` case with a probe of `violations()` (result `[]`). The `apply` case is the same class. It
was found during the SMA-739 intake. Sven decided to close both in this issue.

SMA-738 closed the same class for the build steps `lock` and `stage` with an exact pin of the
whole `run:` text (P25, `LOCK_RUN`, `STAGE_RUN`).

## 2. Goals

- G1. `pin_check.py` refuses any change to the `run:` text of the `propose` steps `verify` and
  `apply`. This includes the four forms in section 1.
- G2. The self-test has fixture rows that prove G1 for each of the four forms, in each of the
  two steps.
- G3. `--negative-control` has one mutation of the REAL workflow for each of the two steps.
- G4. The README no longer lists the `verify` gap as open, and it describes the new rule.

## 3. Non-goals

- The workflow scripts do not change. Only `pin_check.py`, its fixture and the README change.
- The other `propose` steps (`commit`, `base`, `push`, `pr`, `close`) are not pinned. They run no
  `lockstep_check.py` command, so an early exit in them skips no check. An early exit in them
  can only stop the job before a push or a pull request.
- P7 and P18 do not change. They give a more specific message for their cases, and they stay
  as a second check.

## 4. Design

### 4.1 Approach

Pin the whole `run:` text of the two steps exactly. This is the P25 method. A deny-list of shell
forms cannot close the class: a quoted name, an indirect name or `set -n` gets past it. The
SMA-738 spec gave that reason for the build steps, and it applies here too. A deny-list is
rejected.

### 4.2 Rule id

A new rule, **P26**: "the `propose` checker steps are pinned". P25 is the structure of the
`build` job, so P26 is a separate id. Every "P0-P25" text in `pin_check.py` changes to "P0-P26":

- the module docstring (`pin_check.py:5`)
- the pass message of `main` (`pin_check.py:1149`)

### 4.3 Constants

`VERIFY_RUN` and `APPLY_RUN` hold the `run:` text exactly as PyYAML loads it from the workflow.
They use the same form as `LOCK_RUN` and `STAGE_RUN`: a `"\n".join((…)) + "\n"` of the lines.
The comment above them says: change these constants WITH the workflow.

The fixture already has a module constant `VERIFY_RUN` (`pin_check.py:849`). It is the one-line
fixture form `        run: …\n` that the P18 rows replace. The new pin needs a name that
does not collide. The fixture constant is renamed to `VERIFY_RUN_LINE` (or removed, if the new
fixture no longer needs it), and the pin takes the name `VERIFY_RUN`. The plan fixes the exact
name; the rule is: one name has one meaning.

### 4.4 Check

`_propose_violations` compares `str(step.get("run", ""))` of `verify` and `apply` with the pins.
A difference reports:

```text
P26 the <id> step script differs from the pinned text, first at line N
```

The line-number code in `_build_violations` moves into a small helper. P25 and P26 both use it.
The P25 message text does not change.

A missing step gives an empty text. That text differs from the pin, so P26 fires. P7 also fires
for a missing `verify`, as today.

### 4.5 Fixture

The fixture `verify` step and the fixture `apply` step change to the pinned text, in the same
way as `LOCK_STEP` and `STAGE_STEP` (`_indent(…)`). Today they are one-line forms
(`VERIFY_STEP`, `PROPOSE_RUN = "        run: cp a b\n"`). The one-line forms do not pass P26, so
the base fixture must hold the pinned text, or every row reds.

The existing P18 rows that mutate `verify` and the `status` command (`pin_check.py:771-778`)
are rebuilt on the pinned text. They must still want P18, and P18 must still fire for them.
P26 also fires for them; that is correct, and a row matches if any message starts with its want.

### 4.6 Self-test rows

Ten new rows. Each one inserts a line into the pinned text of the fixture:

| Step | Inserted before the checker command | Want |
|---|---|---|
| `verify` | `exit 0` | P26 |
| `verify` | `exit` | P26 |
| `verify` | `set -n` | P26 |
| `verify` | `set -o noexec` | P26 |
| `apply` | `exit 0` | P26 |
| `apply` | `exit` | P26 |
| `apply` | `set -n` | P26 |
| `apply` | `set -o noexec` | P26 |
| `verify` | (one character changed in a path argument) | P26 |
| `apply` | (one character changed in the file list) | P26 |

Each of the eight insertion rows must PASS P7 and P18 on the current code, so that the row shows
the gap. The red-first step in section 6 measures this.

### 4.7 Negative control

Two new mutations of the real workflow, in the same form as the SMA-738 `build_run` helper
(a `propose_run` helper, or `build_run` with a job argument):

- `exit 0` inserted before the `artifact` line of `verify` → wants P26.
- `set -n` inserted before the `status` line of `apply` → wants P26.

The count message changes from `10 mutations` to `12 mutations`.

### 4.8 README

In `ci/wasm-lockstep/README.md`:

- Add a P26 row to the rule table (near line 134).
- Remove the `verify` gap bullet from "What the checks do not prove" (lines 243-245).
- Where the README names the rules that the checks depend on (near line 195, "P18, P25"), add
  P26 if the sentence is about the `propose` job.

## 5. Error handling

No new error path. A missing or non-string `run:` gives a text that differs from the pin, so P26
fires. The exit codes stay: 0 pass, 3 assertion, 2 infrastructure.

## 6. Testing

1. **Red first.** Add the eight insertion rows before the P26 check. Run
   `python3 ci/wasm-lockstep/pin_check.py --self-test`. Record that all eight rows FAIL (got `[]`
   or no P26). This proves the rows show the gap.
2. Add the P26 check. Re-run the self-test: all rows pass.
3. **Delete-the-feature check.** Remove the P26 comparison for one step at a time, and record that
   the rows for that step red. Restore it.
4. `python3 ci/wasm-lockstep/pin_check.py .github/workflows/wasm-lockstep.yml` → `satisfies
   P0-P26`, rc 0.
5. `python3 ci/wasm-lockstep/pin_check.py --negative-control .github/workflows/wasm-lockstep.yml`
   → `12 mutations, 0 failed`, rc 0.
6. `moon run repo:wasm-lockstep`, and the full-graph `moon ci` from the root `CLAUDE.md` before
   the push.

## 7. Risks

- **Maintenance cost.** A later edit to the `verify` or `apply` script must change the pin in the
  same commit, or `repo:wasm-lockstep` reds. This is the same cost as P25, and it is intended.
  The README states it.
- **YAML form.** The pin is the text as PyYAML loads it. A change of the YAML block style
  (`|` to `|-`) changes the trailing newline, and P26 reds. This is intended.
