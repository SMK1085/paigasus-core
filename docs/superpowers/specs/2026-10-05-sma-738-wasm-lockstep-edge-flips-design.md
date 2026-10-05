# SMA-738 — wasm-lockstep: accept a dependency-edge flip between locked versions

Linear: SMA-738. Related: SMA-693 (the workflow and its gate).
Status: revised after the adversarial challenge, 2026-10-05. Sven folded in the run-1 lock
compare (4.5) at Gate 1.

## 1. Problem

The `wasm-lockstep` workflow fails on `main` although the wasm-bindgen family is current.
Run 37319724343 (`workflow_dispatch` on `1d3eaee7`) stopped with:

```
lockstep_check: REFUSED R-NONFAMILY: a package outside the wasm-bindgen family changed, was added or was removed: ['errno', 'quinn-udp', 'rustix', 'tempfile', 'winapi-util']. Nothing was pushed.
```

The expected result was exit 4 ("family current"). A real family bump fails in the same way.

Root cause (MEASURED in the issue, cargo 1.95.0): `container.sh update` runs
`cargo update -p wasm-bindgen -p js-sys -p web-sys -p wasm-bindgen-futures`. On the current lock
no version moves. But the call changes one dependency edge in five packages, from
`windows-sys 0.61.2` to `windows-sys 0.52.0`. Both versions are already in the lock.
`lock_verdict` compares the `dependencies` of non-family packages with the version suffix, so it
refuses with `R-NONFAMILY` before it can reach NoChange.

## 2. Decision

Sven chose option 1 of the issue: compare non-family dependency edges by name. Option 2 (a
separate commit of the canonical lock) was rejected: a later Dependabot or manual `cargo update`
can flip the edges back, and the failure then comes back.

## 3. Goals and non-goals

Goals:

- G1. A run on a `main` whose family is current exits 4 and prints the `family-current` lines.
- G2. A family bump that also flips edges between locked versions passes.
- G3. A non-family package that is added, removed or changed in version, source or checksum still
  refuses with `R-NONFAMILY`. A new or removed dependency name still refuses with `R-NONFAMILY`.
- G4. A moved edge must be the exact ref string that cargo writes for a package in the lock.
  Otherwise the check refuses with `R-EDGE`.
- G5. The reviewer of a bump PR sees every moved edge. The log of a no-change run shows every
  moved edge too.
- G6. The lock that `propose` checks is byte-identical to the lock that `cargo update` wrote in
  container run 1. Code that runs in container run 2 cannot change the lock (4.5).

Non-goals:

- No change to `container.sh`.
- No change to the exit codes.
- No change to the `propose` job.
- No separate commit of the canonical lock (option 2). The edge flips reach `main` only inside a
  merged bump PR.

## 4. Design

Sections 4.1 to 4.4 change `ci/wasm-lockstep/lockstep_check.py` only. Section 4.5 also changes
`.github/workflows/wasm-lockstep.yml` and `ci/wasm-lockstep/pin_check.py`.

### 4.1 The comparison

- The package key stays `(name, version, source)`. The non-family package sets of the two locks
  must be equal. An added, removed or version-changed non-family package still gives
  `R-NONFAMILY`.
- `normalised(entry)` replaces `dependencies` with the list of bare names of all refs (the text
  before the first space), **in the original order**. Family refs were already bare names, so one
  rule now covers all refs. The other fields of the entry (`checksum`, `source`) stay compared
  exactly.
- Why the original order is enough: cargo sorts the refs by the full ref string. A space sorts
  before every character that a crate name can hold, so a version change never changes the order
  of the bare names. A list compare also keeps the count of each name.
- `normalise_ref` and its docstring change to say this: every ref becomes its bare name.

### 4.2 The new refusal `R-EDGE`

**The canonical ref table.** For each lock, the checker makes a dict from the canonical ref
string to the package key. It uses cargo's minimal-form rule:

| Condition | Canonical ref |
|---|---|
| The lock holds one package of that name | `name` |
| The lock holds one package with that name and version | `name version` |
| Otherwise | `name version (source)` |

The version is the text of the `version` field, so build metadata (`1.1.1+spec-1.1.0`, which is
in the lock today) and a pre-release are accepted. There is no ref parser and no semver rule.

**Moved refs.** For each non-family package that is in both locks, the moved refs are the
multiset difference of the raw ref strings: old minus new (removed) and new minus old (added).
Refs to a family name are included (see below).

**The checks.** Each one gives `R-EDGE`:

1. A `dependencies` list of a non-family package in the new lock holds the same ref string two
   times. Cargo never writes this.
2. An added ref is not a key of the new lock's canonical ref table (exact string lookup). This
   refuses an unlocked version, extra text, a Markdown payload, and a wrong form (for example a
   bare name when two versions exist).
3. A removed ref is not a key of the old lock's canonical ref table.
4. One (package, dependency name) has more than one removed or more than one added ref. The
   pairing of old and new refs is then not defined.

The message names the package as `name version` and prints the ref with `!r`, so control
characters are visible.

**Family refs.** A moved ref to a family name gets checks 1 to 3 too. Before this change, a
family ref was only reduced to its bare name, so a payload after a family name passed. A
family ref produces no edge row: the family table already shows the family move.

`R-EDGE` runs directly after the `R-NONFAMILY` check, before the family checks and before the
NoChange decision. So an unresolved edge refuses also when no family package moved.

### 4.3 The report

- An edge row is `(package, package version, dependency, old version, new version)`. The
  versions come from the resolved package keys, not from the ref text. Because the non-family
  key sets are equal, all five values are text from the old lock (the checked-out `main`). Rows
  are sorted.
- `lock_verdict` returns a small result object with two lists: `family` (the existing rows) and
  `edges`. When no family package moved, it raises `NoChangeError`, also when edges moved. The
  `NoChangeError` carries the edge rows.
- `lock` prints one `edge-moved <package> <package version> <dependency> <old> <new>` line per
  edge row, after the `family-moved` lines. On exit 4 it prints the `edge-moved` lines after the
  `family-current` lines. The workflow reads only the `family-current wasm-bindgen` line
  (`wasm-lockstep.yml:120`) and the exit code, so it needs no change. The weekly log then shows
  when cargo keeps flipping edges.
- `artifact` prints the same `edge-moved` lines. The `R-NOCHANGE` message in `run_artifact`
  changes from "the artifact lock equals the checked-out lock" to "the wasm-bindgen family did
  not move", because the two locks can now differ in edges only.
- `body_for` adds a section "Dependency edges that moved" with a table
  `| Package | Dependency | Old | New |` when there is at least one edge row. The Package cell is
  `` `name version` ``, because the lock can hold two versions of one package. With no edge row,
  the body is the same as before.
- The checklist line "Read the `rs/Cargo.lock` diff. Every changed entry is a family package
  from crates.io." becomes: "Read the `rs/Cargo.lock` diff. Every changed package entry is a
  family package from crates.io. Another entry may change only a dependency edge, to a version
  that is already in the lock (the second table). The checker does not read the manifests:
  `cargo-lock-integrity` in `CI` refuses an edge that a manifest does not allow."
- The title does not change.

### 4.4 Data flow and the trust model

```
container run 1: cargo update -p ...      -> work/rs/Cargo.lock
host:            lockstep_check lock      -> rc 0 (family-moved + edge-moved) | 4 | 3 | 2
container run 2: generate-wasm            -> (third-party code runs, with /work writable)
stage step:      cp work/rs/Cargo.lock    -> the artifact
propose:         lockstep_check artifact  -> the same verdict, the PR title and body files
```

**Without 4.5, this change weakens one property of the trust model.** 4.5 removes the weakness.
The stage step copies `rs/Cargo.lock` AFTER container run 2. Run 2 executes build scripts, proc-macros, pnpm packages and `wasm-pack`.
Before this change, the exact comparison in `verify` refused any change that run-2 code made to a
non-family entry. After this change, run-2 code can re-point any non-family edge to another
version that is already in the lock, and `verify` passes. "Already in the lock" does not mean
"already compiled for this target": the new target can be a crate that only a dev-dependency or a
Windows-only path used before.

What limits this:

- No new package, version, source or checksum can enter the lock (4.1).
- Every moved edge is listed in the PR body, with values from the old lock (4.3).
- `cargo-lock-integrity` in `CI` runs on the bot PR and refuses an edge that a manifest does not
  allow. It does not catch an edge move inside the allowed range.
- Run-2 code can already write any bytes into the five wasm artifacts. The added exposure is
  small compared with that.

The text that reaches the PR body is still only package names and versions from the old lock.

The same gap exists on `main` today for the family entries: run-2 code can change a family entry
to another crates.io version with a valid checksum, and `verify` accepts it, because `verify`
compares only with the old lock. 4.5 closes both gaps.

### 4.5 The run-1 lock compare

**The snapshot.** `lockstep_check.py lock` gets an optional `--snapshot <path>`. The `lock`
command reads the new lock ONCE as bytes, decodes and parses those bytes, and runs the verdict.
On exit 0 only, it writes the same bytes to `<path>`. On exit 4, 3 or 2 it writes no file. So the
snapshot is exactly the bytes of the verdict: there is no second read and no `cp` between the
check and the copy. A write error is exit 2.

The `lock` step of the build job passes `--snapshot "$RUNNER_TEMP/run1.lock"`. The volume of both
container runs is `$RUNNER_TEMP/work:/work`, so container run 2 cannot reach
`$RUNNER_TEMP/run1.lock`.

**The compare.** A new subcommand `lockstep_check.py same --a <path> --b <path>` reads both files
as bytes. Equal bytes: exit 0, and it prints `same: the lock did not change after the lock
verdict`. Different bytes: `R-RUN2` (exit 3), with the two sizes and the SHA-256 of each file in
the message. A file that cannot be read: exit 2. It compares bytes, not parsed TOML, so a
whitespace or comment change refuses too.

The `stage` step of the build job runs, as its LAST whole command, after the copy loop:

```
python3 ci/wasm-lockstep/lockstep_check.py same --a "$RUNNER_TEMP/run1.lock" --b "$dst/rs/Cargo.lock"
```

It compares the STAGED copy, which is the file that `upload` sends to `propose`. A refusal fails
the `stage` step, so `upload` and `propose` do not run. The words `python3` and the checker path
are already on the build allowlist (`pin_check.py:97`), so P5 does not change.

**Why `same` is in the checker and not `cmp`.** `cmp` is not on the build allowlist. A
subcommand keeps the compare inside the self-tested checker, and P18 then guards it.

**The pin rules (`pin_check.py`).**

- `same` is added to `CHECKER_SUBS`. So P18 applies: the `same` command must be the last whole
  command of its step, with no `||`, `&&`, `;`, pipe or open `if` around it.
- New rule P25:
  - The build steps with the ids `update`, `lock`, `build`, `stage` and `upload` exist and come
    in that order.
  - The `lock` step runs `python3 ci/wasm-lockstep/lockstep_check.py lock` with
    `--snapshot "$RUNNER_TEMP/run1.lock"`.
  - The `stage` step runs `python3 ci/wasm-lockstep/lockstep_check.py same` with
    `--a "$RUNNER_TEMP/run1.lock"` and `--b "$dst/rs/Cargo.lock"`.
  - No other step of either job names `run1.lock`. So no step can write the snapshot again
    between `lock` and `stage`.
- `FIXTURE` in `pin_check.py` gets the `--snapshot` option on its `lock` step and new `build` and
  `stage` steps, so the fixture passes P25.

**What 4.5 restores.** With 4.5, an edge move can come only from `cargo update` in container
run 1. `container.sh` states that run 1 runs no build script. The weakness in 4.4 then goes
away, and the family-entry gap on `main` closes too. The edge table in the PR body stays: it
shows what cargo's resolution did.

**What 4.5 does not cover.** Run 1 itself (`cargo update`) still runs in the container with
`/work` writable. Its output is the input of the verdict, as before. A compromised cargo or
crates.io index is out of scope, as before.

## 5. Testing

Every new self-test row that checks more than an outcome code uses a check function that raises
`RefusalError("R-SELFTEST", …)` on a wrong value, the same way as `_files_rows`
(`lockstep_check.py:485-496`). `_outcome` alone returns only `PASS`, so it cannot prove a value.

### 5.1 Self-test rows (new)

The fixtures use the names and versions of the real case: `windows-sys 0.61.2` and `0.52.0`, and
the five packages `errno 0.3.14`, `quinn-udp 0.5.15`, `rustix 1.1.5`, `tempfile 3.27.0` and
`winapi-util 0.1.11`. Two versions of `windows-sys` are in the lock, so cargo writes the refs in
the `name version` form.

| Row | Expected |
|---|---|
| The real five-package edge flip, with a family move | `PASS`, and the verdict holds exactly five edge rows with the correct values |
| The real five-package edge flip, no family move | `NOCHANGE`, and the `NoChangeError` carries exactly five edge rows |
| An edge flip and a version change of a non-family package | `R-NONFAMILY` |
| An edge flip and a new dependency name | `R-NONFAMILY` |
| An added ref to a version that is not in the lock, with a family move | `R-EDGE` |
| The same unlocked ref, no family move | `R-EDGE` (proves `R-EDGE` runs before NoChange) |
| The same unlocked ref, with a family version that fails `R-SEMVER` | `R-EDGE` (proves the order) |
| An added ref with a Markdown payload after a valid ref (`windows-sys 0.52.0 [x](http://evil)`) | `R-EDGE` |
| An added bare-name ref when two versions of the name are in the lock | `R-EDGE` |
| A `name version (source)` ref when two packages share the name and version: the correct source | `PASS` (positive twin) |
| The same, with a source that is not in the lock | `R-EDGE` |
| A ref with build metadata (`toml_datetime 1.1.1+spec-1.1.0`) moved between two locked versions | `PASS` |
| A removed ref that is not in the old lock's table | `R-EDGE` (old side) |
| A new `dependencies` list with the same ref two times | `R-EDGE` |
| Two moved refs of one dependency name on one side of one package | `R-EDGE` |
| An added family ref with a payload after the family name | `R-EDGE` |
| The body after a pass with the five-package flip | holds the heading and exactly five rows, one is `` | `errno 0.3.14` | `windows-sys` | `0.61.2` | `0.52.0` | `` |
| The body after a pass with no edge flip | holds no "Dependency edges that moved" heading |

### 5.2 A changed self-test row

"reference form `name version` to a NON-family package" changes `anyhow 1.0.98` to
`anyhow 1.0.99`. `1.0.99` is not in the lock. The expected code changes from `R-NONFAMILY` to
`R-EDGE`. The row label and the `_ref_rows` docstring change too: the row now proves that an
unlocked non-family ref refuses with `R-EDGE`. The old claim "the normalisation is scoped to
FAMILY names" is no longer true.

### 5.3 The negative control on the real lock

New rows in `negative_control`, on HEAD's lock:

- **Target selection (deterministic).** The first non-family package in lock order that has a
  ref to a name with two or more versions in the lock, where the package does not already refer
  to the other version. The ref is re-pointed to the canonical ref of the other version. If no
  candidate exists, the control raises `InfraError`, the same way as the existing
  `lockstep_check.py:577-579`. The real lock has more than 30 names with two or more versions, so
  "no candidate" means that the finder is broken.
- **Row 1.** The re-point alone: `NOCHANGE`, and the `NoChangeError` carries exactly one edge
  row with the expected values.
- **Row 2.** The re-point plus the existing "every family patch version moved" mutation:
  `PASS`, with exactly one edge row.
- **Row 3.** The same ref re-pointed to `<name> 999.0.0`: `R-EDGE`.

The existing rows stay.

### 5.4 The real reproduction (before the PR)

On a scratch copy (`git archive`) of the branch head, with cargo 1.95.0 (record `cargo --version`
in the PR body):

1. Run `cargo update -p wasm-bindgen -p js-sys -p web-sys -p wasm-bindgen-futures` in `rs/`.
2. Run the new `lockstep_check.py lock --old <HEAD lock> --new <updated lock>`.
3. Expected: exit 4 with the seven `family-current` lines and the five `edge-moved` lines. If a
   new family release appears before the run, exit 0 with `family-moved` lines and the five
   `edge-moved` lines is also correct. In both cases there is no `R-NONFAMILY` and no `R-EDGE`.
4. Run the old checker (`main`) on the same two locks. Expected: exit 3 with `R-NONFAMILY` and
   the five names. This proves that the reproduction shows the bug.

The `workflow_dispatch` run on `main` after the merge is the final proof (issue acceptance 1).
The `propose` job needs the `release-pr` environment, which only `main` can use, so a run on the
feature branch cannot prove the whole workflow.

### 5.5 Tests for 4.5

`lockstep_check.py` self-test rows:

| Row | Expected |
|---|---|
| `same` on two equal files | `PASS` |
| `same` on two files that differ by one byte | `R-RUN2` |
| `same` on two files that differ only in a trailing newline | `R-RUN2` |
| `same` with a missing file | `INFRA` |
| `lock --snapshot` on a pass | the snapshot exists and is byte-equal to the new lock |
| `lock --snapshot` on a refusal, and on no change | no snapshot file exists |

`lockstep_check.py --negative-control` rows on HEAD's lock: `same` of the lock and itself is
`PASS`; `same` of the lock and the lock with the first `version` changed is `R-RUN2`.

`pin_check.py` mutation rows (each must give exactly its rule code):

| Mutation | Expected |
|---|---|
| The `stage` step without the `same` command | P25 |
| The `same` command with `\|\| true` joined to it | P18 |
| The `same` command not the last command of the step | P18 |
| The `lock` step without `--snapshot` | P25 |
| `--snapshot` to another path | P25 |
| The `stage` step before the `build` step | P25 |
| Another step that writes `run1.lock` (`cp x "$RUNNER_TEMP/run1.lock"` in the `build` step) | P25 |
| The `--b` argument names the work copy in place of the staged copy | P25 |

`pin_check.py --negative-control` on the real workflow passes, and the existing rule rows stay.

**The scratch-branch run (M5, before the PR).** The stage step runs only when `changed=true`, and
the family on `main` is current. So the real proof is a scratch-branch run in the pattern of M0:

1. Make `feature/sma-738-m5-scratch` from the feature branch. It never merges.
2. In its `rs/Cargo.lock`, replace the seven family entries with the 0.2.128 entries of
   `c9df6f09` and the family refs of the other packages with the refs of `c9df6f09`. Check
   locally with cargo 1.95.0 that the four-package `cargo update -p` then moves the seven family
   packages, flips the five edges, and gives exit 0 with the new checker.
3. Push the branch and run `gh workflow run wasm-lockstep.yml --ref feature/sma-738-m5-scratch`.
4. Expected: the `build` job passes. The `lock` step prints seven `family-moved` and five
   `edge-moved` lines. The `stage` step prints the `same:` line. `propose` does not run its steps,
   because the `release-pr` environment allows only `main`.
5. Record the run URL and the values in the README under "M5".
6. Delete the scratch branch after the run.

A negative proof of `R-RUN2` on a runner needs a hostile run 2. That is not done: the self-test
and the negative control prove the refusal, and P25 proves the wiring.

Steps 3 and 6 push to GitHub and start a workflow. They need Sven's approval when they run.

### 5.6 The gates

`./ci/wasm-lockstep/run.sh --self-test`, `--negative-control` and the real run must pass.
`repo:ruff-ci` must pass on the changed Python file.

## 6. Documents

- `ci/wasm-lockstep/README.md`:
  - The `R-NONFAMILY` row: dependency refs are compared by bare name, in order.
  - A new `R-EDGE` row and a new `R-RUN2` row (for `same`).
  - The P25 row in the pin-rules table, and `same` in the P18 row.
  - The trust model: the host snapshots the lock after the verdict, and the stage step refuses a
    lock that run 2 changed.
  - A new measurement section "M5" with the scratch-branch run.
  - A sentence on the `edge-moved` output (also on exit 4) and the second PR-body table.
  - "What the checks do not prove": `cargo update` in run 1 can re-point a non-family edge to
    another version that is already in the lock. This can change which code a target compiles.
    No new package enters the lock. The reviewer reads the edge table. `cargo-lock-integrity`
    catches only an edge outside the manifest range. A negative proof of `R-RUN2` on a runner
    was not done.
- `rs/CLAUDE.md`, the runbook "The wasm-bindgen family does not move through dependabot":
  - The diff comment `# the family entries (seven at M0) plus any new dep` adds "and dependency
    edges re-pointed between locked versions".
  - The list of refusal causes adds `R-EDGE`. The recovery for an `R-EDGE` with no family move:
    the lock on `main` holds a ref that cargo now writes in another form or the run produced a
    ref that is not in the lock. Run the four-package `cargo update -p` locally, read the edge
    diff, and if it is only re-points between locked versions, commit the lock in a normal
    `feature/sma-NNN-<slug>` PR. Otherwise open an issue.
- `docs/superpowers/specs/2026-09-27-sma-693-wasm-bindgen-lockstep-updater-design.md` §5.3: a
  note under the invariant "Every non-family package is identical in both locks" that SMA-738
  replaced it with the bare-name comparison and `R-EDGE`.
- The `lock_verdict` docstring and its return type.

## 7. Risks and rollback

- The check is weaker: an edge flip between locked versions that `cargo update` made passes. The
  PR body lists every flip. With 4.5, run-2 code cannot make such a flip.
- The stage compare (4.5) runs on a runner only when the family moves. M5 proves it once on a
  scratch branch. The next real proof comes with the next wasm-bindgen release.
- A future cargo can write a ref in another form. That ref then is not in the canonical ref table
  and gives `R-EDGE`. This fails closed, and the runbook in `rs/CLAUDE.md` applies.
- Open question (not measured): which cargo version wrote the edges on `main`. If Dependabot's
  cargo writes `0.61.2` again, the flips come back every week. The `edge-moved` lines on exit 4
  make this visible in the log.
- Rollback: revert the PR. The weekly run then refuses with `R-NONFAMILY` again, which is the
  known state before this change. The workflow and `pin_check.py` change in the same commit, so
  a revert keeps them consistent.
