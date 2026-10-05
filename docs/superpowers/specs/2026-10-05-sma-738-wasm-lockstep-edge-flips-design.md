# SMA-738 — wasm-lockstep: accept a dependency-edge flip between locked versions

Linear: SMA-738. Related: SMA-693 (the workflow and its gate).
Status: draft, 2026-10-05.

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

Sven chose option 1 of the issue: compare non-family dependency edges by name. Option 2 (commit
the canonical lock once) was rejected: a later Dependabot or manual `cargo update` can flip the
edges back, and the failure then comes back.

## 3. Goals and non-goals

Goals:

- G1. A run on a `main` whose family is current exits 4 and prints the `family-current` lines.
- G2. A family bump that also flips edges between locked versions passes.
- G3. A non-family package that is added, removed or changed in version, source or checksum still
  refuses with `R-NONFAMILY`. A new or removed dependency name still refuses with `R-NONFAMILY`.
- G4. A moved edge must point to a package that is in the lock. Otherwise the check refuses.
- G5. The reviewer of a bump PR sees every moved edge.

Non-goals:

- No change to `container.sh`, `.github/workflows/wasm-lockstep.yml` or `pin_check.py`.
- No change to the exit codes or to the trust model.
- No commit of the edge flips to `main`.

## 4. Design

All code changes are in `ci/wasm-lockstep/lockstep_check.py`.

### 4.1 The comparison

- The package key stays `(name, version, source)`. The non-family package sets of the two locks
  must be equal. An added, removed or version-changed non-family package still gives
  `R-NONFAMILY`.
- `normalised(entry)` replaces `dependencies` with the **sorted list of bare names** of all refs
  (the text before the first space). Family refs were already bare names, so one rule now covers
  all refs. The sorted list is a multiset, so a count change of one name still refuses. The other
  fields of the entry (`checksum`, `source`) stay compared exactly.
- `normalise_ref` and its docstring change to say this: every ref becomes its bare name.

### 4.2 The new refusal `R-EDGE`

Each non-family package that is in both locks gets its moved refs: the multiset difference of the
raw ref strings, old minus new and new minus old, without refs to a family name.

Each moved ref in the NEW lock must resolve to a package of the new lock:

| Ref form | Resolves when |
|---|---|
| `name` | The new lock holds exactly one package of that name. |
| `name X.Y.Z` | `X.Y.Z` is a strict semver (`SEMVER.fullmatch`) and the new lock holds exactly one package with that name and version. |
| `name X.Y.Z (source)` | The semver rule above, and the new lock holds the key `(name, X.Y.Z, source)`. |

Any other text (an extra word, a missing `)`, a non-semver version) does not resolve. A ref that
does not resolve gives `R-EDGE` with the package and the ref in the message. The message prints
the ref with `!r`, so control characters are visible.

Each moved ref in the OLD lock is resolved the same way against the old lock. The old lock comes
from the checked-out `main`. If an old ref does not resolve, that is also `R-EDGE`.

Why `R-EDGE` exists: the new lock comes from a container that ran third-party code. The
bare-name check alone accepts any text after the correct name, for example `windows-sys 9.9.9`
or a Markdown payload. The edge rows go into the PR body (4.3). The resolution check makes sure
that a moved edge points to code that is already in the lock, and that only a semver reaches the
body.

`R-EDGE` runs directly after the `R-NONFAMILY` check and before the family checks.

### 4.3 The report

- An edge row is `(package, package version, dependency, old version, new version)`. The
  versions come from the resolved packages, not from the ref text. So a bare-name ref also gets a
  version. Rows are sorted.
- `lock_verdict` returns a small result object with two lists: `family` (the existing rows) and
  `edges`. The NoChange rule does not change: when no family package moved, it raises
  `NoChangeError`, also when edges moved. The workflow then discards the work copy.
- `lock` prints one `edge-moved <package> <package version> <dependency> <old> <new>` line per
  edge row, after the `family-moved` lines. The workflow reads only the `family-current
  wasm-bindgen` line (`wasm-lockstep.yml:120`) and the exit code, so it needs no change.
- `artifact` prints the same `edge-moved` lines.
- `body_for` adds a section "Dependency edges that moved" with a table
  `| Package | Dependency | Old | New |` when there is at least one edge row. With no edge row,
  the body is the same as before.
- The checklist line "Read the `rs/Cargo.lock` diff. Every changed entry is a family package
  from crates.io." becomes: "Read the `rs/Cargo.lock` diff. Every changed package entry is a
  family package from crates.io. Another entry may change only a dependency edge, to a version
  that is already in the lock (the second table)."
- The title does not change.

### 4.4 Data flow

```
container: cargo update -p ...      -> work/rs/Cargo.lock
host:      lockstep_check lock        -> rc 0 (family-moved + edge-moved) | 4 | 3 | 2
propose:   lockstep_check artifact    -> the same verdict, the PR title and body files
```

Nothing new crosses a trust boundary. The edge rows hold only package names from the old lock
and versions that passed `SEMVER.fullmatch`.

## 5. Testing

### 5.1 Self-test rows (new)

| Row | Expected |
|---|---|
| An edge flip between two locked versions, with a family move | `PASS`, and the verdict holds exactly one edge row with the correct five values |
| The same edge flip, no family move | `NOCHANGE` |
| The `dependencies` of a non-family package in another order | `NOCHANGE` |
| An edge flip and a version change of a non-family package | `R-NONFAMILY` |
| An edge flip and a new dependency name | `R-NONFAMILY` |
| An edge to a version that is not in the lock | `R-EDGE` |
| An edge with a Markdown payload after a valid name (`windows-sys 0.52.0 [x](http://evil)`) | `R-EDGE` |
| An edge to a bare name that has two versions in the lock | `R-EDGE` |
| An edge in the `name X.Y.Z (source)` form with a source that is not in the lock | `R-EDGE` |
| The body after a pass with an edge flip | holds the edge table row `` | `errno` | `windows-sys` | `0.61.2` | `0.52.0` | `` |
| The body after a pass with no edge flip | holds no "Dependency edges that moved" heading |

The fixtures use names and versions of the real case (`errno 0.3.14`, `windows-sys 0.61.2` and
`0.52.0`).

### 5.2 A changed self-test row

"reference form `name version` to a NON-family package" changes `anyhow 1.0.98` to
`anyhow 1.0.99`. `1.0.99` is not in the lock. The expected code changes from `R-NONFAMILY` to
`R-EDGE`. The row still proves that the family normalisation does not hide a non-family change.

### 5.3 The negative control on the real lock

A new row in `negative_control`: take HEAD's lock, find a non-family package with a ref to a name
that has two or more versions in the lock, and re-point that ref to another of those versions.
Expected: `NOCHANGE`. If the real lock has no such name, the row prints
`skip  <label>: no name with two locked versions` on stdout and does not count as a failure. The
existing rows stay.

### 5.4 The real reproduction (before the PR)

On a scratch copy (`git archive`) of the branch head, with cargo 1.95.0:

1. Run `cargo update -p wasm-bindgen -p js-sys -p web-sys -p wasm-bindgen-futures` in `rs/`.
2. Run `lockstep_check.py lock --old <HEAD lock> --new <updated lock>`.
3. Expected: exit 4 and the seven `family-current` lines. Record the output in the PR body.
4. Run the old checker (`main`) on the same two locks. Expected: exit 3 with `R-NONFAMILY` and
   the five names. This proves the reproduction shows the bug.

The `workflow_dispatch` run on `main` after the merge is the final proof (issue acceptance 1).
The `propose` job needs the `release-pr` environment, which only `main` can use, so a run on the
feature branch cannot prove the whole workflow.

### 5.5 The gates

`./ci/wasm-lockstep/run.sh --self-test`, `--negative-control` and the real run must pass.
`repo:ruff-ci` must pass on the changed Python file.

## 6. Documents

- `ci/wasm-lockstep/README.md`:
  - The `R-NONFAMILY` row: dependency refs are compared by bare name.
  - A new `R-EDGE` row.
  - A sentence on the `edge-moved` output and the second PR-body table.
  - "What the checks do not prove": a non-family package can re-point an edge to another version
    that is already in the lock. This changes which code a target compiles (for example, on
    Windows), but no new code enters the lock. The reviewer reads the edge table.

## 7. Risks

- The check is weaker: an edge flip between locked versions passes. The resolution check (4.2)
  limits it to code that is already in the lock, and the PR body lists it.
- A future cargo can write a ref in a fourth form. That ref then does not resolve and gives
  `R-EDGE`. This fails closed. The manual runbook in `rs/CLAUDE.md` applies.
