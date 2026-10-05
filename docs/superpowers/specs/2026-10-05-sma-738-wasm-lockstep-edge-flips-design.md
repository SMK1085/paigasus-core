# SMA-738 — wasm-lockstep: accept a dependency-edge flip between locked versions

Linear: SMA-738. Related: SMA-693 (the workflow and its gate).
Status: revised after the adversarial challenge, 2026-10-05.

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

Non-goals:

- No change to `container.sh`, `.github/workflows/wasm-lockstep.yml` or `pin_check.py`.
- No change to the exit codes.
- No separate commit of the canonical lock (option 2). The edge flips reach `main` only inside a
  merged bump PR.

## 4. Design

All code changes are in `ci/wasm-lockstep/lockstep_check.py`.

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

**This change weakens one property of the trust model.** The stage step copies `rs/Cargo.lock`
AFTER container run 2. Run 2 executes build scripts, proc-macros, pnpm packages and `wasm-pack`.
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

A follow-up issue (proposed, not part of this change) restores the old property: the host keeps a
copy of the run-1 lock outside `/work`, and the stage step refuses when `cmp` shows that run 2
changed the lock. That is a workflow change and needs a `pin_check.py` P5 allowlist change.

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

### 5.5 The gates

`./ci/wasm-lockstep/run.sh --self-test`, `--negative-control` and the real run must pass.
`repo:ruff-ci` must pass on the changed Python file.

## 6. Documents

- `ci/wasm-lockstep/README.md`:
  - The `R-NONFAMILY` row: dependency refs are compared by bare name, in order.
  - A new `R-EDGE` row.
  - A sentence on the `edge-moved` output (also on exit 4) and the second PR-body table.
  - "The trust model": the lock that `propose` checks is copied after container run 2 (4.4).
  - "What the checks do not prove": run-2 code or cargo can re-point a non-family edge to another
    version that is already in the lock. This can change which code a target compiles. No new
    package enters the lock. The reviewer reads the edge table. `cargo-lock-integrity` catches
    only an edge outside the manifest range.
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

- The check is weaker: an edge flip between locked versions passes, also when run-2 code made it
  (4.4). The PR body lists every flip.
- A future cargo can write a ref in another form. That ref then is not in the canonical ref table
  and gives `R-EDGE`. This fails closed, and the runbook in `rs/CLAUDE.md` applies.
- Open question (not measured): which cargo version wrote the edges on `main`. If Dependabot's
  cargo writes `0.61.2` again, the flips come back every week. The `edge-moved` lines on exit 4
  make this visible in the log.
- Rollback: revert the PR. The weekly run then refuses with `R-NONFAMILY` again, which is the
  known state before this change.
