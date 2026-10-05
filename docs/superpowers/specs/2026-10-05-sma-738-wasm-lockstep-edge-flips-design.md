# SMA-738 — wasm-lockstep: accept a dependency-edge flip between locked versions

Linear: SMA-738. Related: SMA-693 (the workflow and its gate).
Status: revised after two adversarial challenges, 2026-10-05. Sven folded in the run-1 lock
compare (4.5) at Gate 1. The second challenge replaced the snapshot file with a step output.

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
- G6. A lock that container run 2 changed does not reach `propose`: the `stage` step refuses it.
  The lock that `upload` sends is byte-identical to the lock that the `lock` step checked (4.5).

Non-goals:

- No change to `container.sh`.
- No change to the exit codes.
- No change to the `propose` job and no new job output (P9 stays as it is).
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
host, lock:      lockstep_check lock      -> rc 0 (family-moved + edge-moved + lock-sha256) | 4 | 3 | 2
                                             step output lock_sha256 (4.5)
container run 2: generate-wasm            -> (third-party code runs, with /work writable)
host, stage:     cp work/rs/Cargo.lock    -> stage/rs/Cargo.lock
                 lockstep_check same      -> rc 0 | 3 (R-RUN2) | 2      (4.5)
host, upload:    stage/                   -> the artifact
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

**The hash.** The `lock` command reads the new lock ONCE as bytes, through one reader function
that `lock`, `artifact` and `same` all use. The reader opens the file in binary mode. It decodes
the bytes as strict UTF-8 with no newline translation, and parses that text. The `lock` command
then runs the verdict on that one parsed dict. On the no-change path it reuses the same dict for
`current_verdict` (today `main()` reads `--new` a second time, `lockstep_check.py:626`). On
exit 0 only, `lock` prints `lock-sha256 <64 hex>`, the SHA-256 of exactly those bytes.

The `lock` step of the build job extracts that line with
`sed -n 's/^lock-sha256 \([0-9a-f]\{64\}\)$/\1/p'` and, on exit 0 only, writes
`lock_sha256=<hex>` to `$GITHUB_OUTPUT`, next to `changed=true`. This runs on the host, before
container run 2. A step output cannot change after its step ends. So code in run 2 cannot change
the expected value, and there is no file that it could overwrite, redirect or replace with a
symlink. The output is a STEP output, not a job output, so P9 (`build` outputs only `changed`)
does not change. P24 bans only the outputs of `update` and `build`, so it does not object.

**The compare.** A new subcommand
`lockstep_check.py same --sha256 <64 hex> --file <path>`:

- The `--sha256` value must fullmatch `[0-9a-f]{64}`. An empty or malformed value is exit 2.
  So a broken extraction in the `lock` step fails closed: the bump goes red, it is not passed.
- The file is opened with `O_NOFOLLOW` and must be a regular file of `SIZE_CAP` (8 MiB) or less.
  A symlink, another file type or an oversize file is `R-RUN2`. An unreadable file is exit 2.
- The file is hashed in chunks. Equal hash: exit 0, and it prints
  `same: rs/Cargo.lock sha256 <hex> did not change after the lock verdict`. Different hash:
  `R-RUN2` (exit 3), with the expected hash, the actual hash and the file size in the message.
  No byte of the file goes to the log.
- It compares bytes, not parsed TOML, so a whitespace or comment change refuses too.

`artifact` in `propose` also prints `lock-sha256 <hex>` of the downloaded lock. A reviewer can
match the three values (`lock`, `same`, `artifact`) across the job logs.

**The `stage` step.** It keeps its copy loop. Its LAST whole command is:

```
python3 ci/wasm-lockstep/lockstep_check.py same --sha256 "$LOCK_SHA256" --file "$RUNNER_TEMP/stage/rs/Cargo.lock"
```

with the step `env:` `LOCK_SHA256: ${{ steps.lock.outputs.lock_sha256 }}`. The `--file` path is
the literal `$RUNNER_TEMP/stage/...`, not `$dst/...`, so no shell variable assignment can move
it. It compares the STAGED copy. P25 pins that `upload` sends exactly that directory. A refusal
fails the `stage` step, so `upload` and `propose` do not run. The words `python3` and the checker
path are already on the build allowlist (`pin_check.py:97`), so P5 does not change.

**Why `same` is in the checker and not `cmp` or `sha256sum`.** Neither word is on the build
allowlist. A subcommand keeps the compare inside the self-tested checker.

**`_options` refuses a repeated flag** with exit 2 (today it keeps the last value,
`lockstep_check.py:598-609`). Then `same --sha256 A --file B --file C` cannot compare another
file than the one that P25 pins.

**The pin rules (`pin_check.py`).**

- **P18 runs for every job.** Today `_checker_tail` runs only for `propose`
  (`pin_check.py:469-470`). The condition is removed, and `same` is added to `CHECKER_SUBS`. So
  the `same` command must be the last whole command of the `stage` step, with no `||`, `&&`, `;`,
  pipe or open `if` around it. The `lock` step is not affected: `lock` is not in `CHECKER_SUBS`,
  and it keeps its `|| rc=$?`. The `_checker_tail` docstring and the README P18 row change.
- **New rule P25** (the build job):
  - The build step ids are EXACTLY `reclaim, checkout, ref, copy, update, lock, build, stage,
    upload`, in that order (`BUILD_ORDER`, the same pattern as `PROPOSE_ORDER` in P7). A step
    between `stage` and `upload`, or `lock` after `build`, refuses.
  - The `if:` of `build`, `stage` and `upload` is exactly `steps.lock.outputs.changed == 'true'`.
    Without this pin, `stage` could run on exit 4, `same` would then get an empty hash and exit 2,
    and every no-change week would go red.
  - The `lock` step holds the exact command token list
    `python3 ci/wasm-lockstep/lockstep_check.py lock --old "$RUNNER_TEMP/old.lock" --new "$RUNNER_TEMP/work/rs/Cargo.lock"`
    and the exact line `echo "lock_sha256=${sha}" >> "$GITHUB_OUTPUT"`.
  - The `stage` step holds the exact command token list of the `same` command above, and its
    `env:` is exactly `{LOCK_SHA256: "${{ steps.lock.outputs.lock_sha256 }}"}`.
  - `upload.with.path` is exactly `${{ runner.temp }}/stage/`. This closes the README gap "an
    upload of `$RUNNER_TEMP/work/` in place of the stage directory" (`README.md:151-152`).
  - No step of either job assigns `RUNNER_TEMP` (a shell assignment or an `env:` key).
  - The exact token lists also refuse a repeated flag in the workflow text.
- `FIXTURE` in `pin_check.py` gets the nine build steps with their ids, the `if:` values, the
  `lock_sha256` line, the `stage` `env:` and command, and `upload.with.path`, so that it passes
  P25. "P0-P24" becomes "P0-P25" (`pin_check.py:5`, `:921`).

**Where the control is.** 4.5 puts a control in the `build` job that `propose` cannot check
again. Until now, the workflow comments said that the `lock` step "is not the control"
(`wasm-lockstep.yml:106-108`, SMA-693 spec 5.1 step 5). That stays true for the lock VERDICT:
`propose` runs the verdict again. It is not true for G6: only `stage` checks that run 2 did not
change the lock. G6 therefore depends on `pin_check.py` (P25) and on the integrity of the build
runner host. A container escape in run 2 can reach the runner and defeat G6. That is the same
residual that Sven accepted for the cache scope (SMA-693 Q7). A check in `propose` would need the
hash as a second job output, which conflicts with P9 and with the non-goal "no change to
`propose`". So it is not part of this change.

**The assumption that run 2 does not write the lock.** wasm-pack makes its own unlocked cargo
call before its `--locked` build, and it repairs an inconsistent lock there
(`ts/packages/paigasus-kernel/moon.yml:164-170`, measured in SMA-601). On a consistent lock it
should write nothing. M0 and M3 prove only that the run-2 lock passed the semantic compare in
`verify`. Nobody compared the BYTES before and after run 2. M5 measures this (5.5). If run 2
does rewrite the lock in a benign way, `same` refuses every bump. The runbook entry in 6 and the
targeted rollback in 7 cover that case.

**What 4.5 restores.** With 4.5, an edge move that reaches `propose` can come only from
`cargo update` in container run 1. `container.sh` states that run 1 runs no build script. The
weakness in 4.4 then goes away, and the family-entry gap on `main` closes too. The edge table in
the PR body stays: it shows what cargo's resolution did.

**What 4.5 does not cover.** Run 1 itself (`cargo update`) still runs in the container with
`/work` writable. Its output is the input of the verdict, as before. A compromised cargo or
crates.io index is out of scope, as before. `pin_check.py` reads command words and exact token
lists. It does not read shell semantics, so the existing limits in the README still apply.

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

The two family-form rows of `_ref_rows` change too. The rows "reference form `name version` to a
family package" and "reference form `name version (source)` to a family package" change from
`PASS` to `R-EDGE`. The reason: in a lock that passes `R-DUPLICATE`, a family name has one version,
so cargo writes the bare name only. The two longer forms are then not canonical, and checks 2 and 3
of 4.2 refuse them. The row "reference form `name` to a family package" stays `PASS`.

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

The new `lockstep_check.py` rows call `main([...])`, so the production argument path is covered.
`main()` RETURNS 4 on no change, and `_outcome` maps every return value to `PASS`
(`lockstep_check.py:378-387`). So each row that needs a return code checks the return value in a
check function that raises `R-SELFTEST`.

| Row | Expected |
|---|---|
| `lock` on a pass | return 0, and the printed `lock-sha256` equals the SHA-256 of the raw file bytes |
| `lock` on a pass, with a lock that has `\r\n` line ends | the printed hash is the hash of the raw bytes (proves no newline translation) |
| `lock` on no change | return 4, and no `lock-sha256` line |
| `lock` on a refusal | `R-...`, and no `lock-sha256` line |
| `same` with the correct hash | `PASS` |
| `same` on a file that differs by one byte | `R-RUN2` |
| `same` on a file that differs only in a trailing newline | `R-RUN2` |
| `same` with a symlink as `--file` | `R-RUN2` |
| `same` with a file over `SIZE_CAP` (a sparse file) | `R-RUN2` |
| `same` with a missing file | `INFRA` |
| `same` with an empty `--sha256` | `INFRA` |
| `same` with an upper-case or 63-character hash | `INFRA` |
| `same` with `--file` given two times | `INFRA` |
| `same` without `--sha256`, and without `--file` | `INFRA` (two rows) |
| `artifact` on a pass | prints the `lock-sha256` of the artifact lock |

`lockstep_check.py --negative-control` rows on HEAD's lock: `same` with the hash of the lock is
`PASS`; `same` with the hash of the lock and a copy that has its first `version` changed is
`R-RUN2`.

`pin_check.py` mutation rows on the FIXTURE (each must give exactly its rule code):

| Mutation | Expected |
|---|---|
| The `stage` step without the `same` command | P25 |
| The `same` command with `\|\| true` joined to it | P18 |
| The `same` command not the last command of the `stage` step | P18 |
| A second `--file` on the `same` command | P25 |
| `--file` names the work copy (`$RUNNER_TEMP/work/rs/Cargo.lock`) | P25 |
| The `stage` `env:` reads another step output | P25 |
| The `lock` step without the `lock_sha256` line | P25 |
| `lock` after `build` | P25 |
| A new step between `stage` and `upload` | P25 |
| The `stage` `if:` changed to `steps.lock.outputs.changed != 'false'` | P25 |
| `upload.with.path` changed to `${{ runner.temp }}/work/` | P25 |
| A step `env:` that sets `RUNNER_TEMP` | P25 |
| A shell assignment `RUNNER_TEMP=/tmp` in the `stage` step | P25 |

`pin_check.py --negative-control` gets three mutations of the REAL workflow file. Today its one
build mutation only adds a secrets read (`pin_check.py:868-906`). The fixture rows prove that the
rule works. Only mutations of the real file prove that it bites on the real structure.

| Mutation of the real workflow | Expected |
|---|---|
| Delete the `same` line | P25 |
| Append `\|\| true` to the `same` line | P18 |
| Delete the `lock_sha256` line | P25 |

**The scratch-branch runs (M5, before the PR).** The `stage` step runs only when
`changed=true`, and the family on `main` is current. The real `build` job refuses every ref other
than `main` (`wasm-lockstep.yml:72-80`). So M5 follows the M0 pattern (SMA-693 plan, M0 task): a
scratch-only workflow, not the reviewed file.

1. Make `feature/sma-738-m5-scratch` from the feature branch. It never merges, and no PR is
   opened for it.
2. On the scratch branch only, add `.github/workflows/wasm-lockstep-m5.yml`: a copy of the
   reviewed `wasm-lockstep.yml` with the `ref` step and the whole `propose` job deleted. Its only
   trigger is `push`, with a block-form `branches:` list that holds only
   `feature/sma-738-m5-scratch`. It has no `workflow_dispatch`: GitHub runs `workflow_dispatch`
   only for a workflow file that exists on the default branch, and this file never reaches
   `main`. M0 used a push trigger for the same reason. Each push to the scratch branch starts one
   run, so each run is one push of one complete commit. It has no `environment`, no secret and no
   App token, so it cannot push to `deps/wasm-bindgen-lockstep`. Record the exact `diff` against
   the reviewed file in the README M5 section.
3. In the scratch branch's `rs/Cargo.lock`, replace the seven family entries with the 0.2.128
   entries of `c9df6f09` and the family refs of the other packages with the refs of `c9df6f09`.
   Why a hand edit and not `cargo update --precise`: a downgrade can flip the edges itself, and
   the old lock then shows no flip. Check locally with cargo 1.95.0 that the four-package
   `cargo update -p` then moves the seven family packages, flips the five edges, and gives exit 0
   with the new checker. Commit the scratch workflow and the scratch lock together, and do not
   push before step 4.
4. **Positive run.** Push the branch. The push starts the run. Find it with
   `gh run list --workflow wasm-lockstep-m5.yml --branch feature/sma-738-m5-scratch` and match its
   `headSha` to the pushed commit. Expected: the run ends green. The `lock` step prints seven `family-moved` lines, five `edge-moved` lines and a
   `lock-sha256` line. The `stage` step prints the `same:` line with the same hash. Record the
   hash, and the SHA-256 of the work lock after run 2, in the README. This answers the open
   question: does run 2 write the lock?
5. **Negative run.** On the scratch branch only, add one line at the end of the `build` case of
   `container.sh`: `printf '\n' >> rs/Cargo.lock`. Commit it and push it. The push starts the
   second run; match it by `headSha` in the same way. Expected: the `stage`
   step refuses with `R-RUN2`, and `upload` does not run. This proves the refusal on the real
   mount, owner and paths, which the self-test cannot do.
6. Record both run URLs and the values in the README under "M5".
7. Delete the remote scratch branch, check with `git ls-remote` that it is gone, and delete the
   local branch with `git branch -D`.

Steps 4, 5 and 7 push to GitHub. The pushes of steps 4 and 5 start the workflow runs. They need
Sven's approval when they run.

### 5.6 The gates

`./ci/wasm-lockstep/run.sh --self-test`, `--negative-control` and the real run must pass.
`repo:ruff-ci` must pass on the changed Python file.

## 6. Documents

- `ci/wasm-lockstep/README.md`:
  - The `R-NONFAMILY` row: dependency refs are compared by bare name, in order.
  - A new `R-EDGE` row and a new `R-RUN2` row (for `same`).
  - The P25 row in the pin-rules table. The P18 row: it now applies to every job, and `same` is
    one of its subcommands.
  - The trust model: a new bullet. The `lock` step writes the hash of the verdict bytes as a step
    output before run 2. The `stage` step refuses a lock that run 2 changed. G6 depends on
    `pin_check.py` and on the build runner. The container-escape residual (Q7) now also covers
    G6.
  - Remove the gap "an upload of `$RUNNER_TEMP/work/` in place of the stage directory" from
    "What the checks do not prove": P25 closes it.
  - A new measurement section "M5" with both scratch-branch runs and the `diff` of the scratch
    workflow.
  - A sentence on the `edge-moved` output (also on exit 4) and the second PR-body table.
  - "What the checks do not prove": `cargo update` in run 1 can re-point a non-family edge to
    another version that is already in the lock. This can change which code a target compiles.
    No new package enters the lock. The reviewer reads the edge table. `cargo-lock-integrity`
    catches only an edge outside the manifest range.
- `rs/CLAUDE.md`, the runbook "The wasm-bindgen family does not move through dependabot":
  - The diff comment `# the family entries (seven at M0) plus any new dep` adds "and dependency
    edges re-pointed between locked versions".
  - The list of refusal causes adds `R-EDGE` and `R-RUN2`.
  - The recovery for `R-RUN2`: run 2 changed the lock. Reproduce run 2 locally in the pinned
    image, as in M2, and diff the lock before and after it. If the change is benign (for example
    a lock repair by wasm-pack), open an issue and use the manual runbook for the bump. If it is
    not benign, treat the family release as hostile and do not merge it. The recovery for an `R-EDGE` with no family move:
    the lock on `main` holds a ref that cargo now writes in another form or the run produced a
    ref that is not in the lock. Run the four-package `cargo update -p` locally, read the edge
    diff, and if it is only re-points between locked versions, commit the lock in a normal
    `feature/sma-NNN-<slug>` PR. Otherwise open an issue.
- `docs/superpowers/specs/2026-09-27-sma-693-wasm-bindgen-lockstep-updater-design.md` §5.3: a
  note under the invariant "Every non-family package is identical in both locks" that SMA-738
  replaced it with the bare-name comparison and `R-EDGE`.
- The `lock_verdict` docstring and its return type.
- The usage docstring of `lockstep_check.py` (`:7-12`): the `same` subcommand.
- `.github/workflows/wasm-lockstep.yml`: the header comment (`:11-18`), the comment of the `lock`
  step (`:106-108`) and the comment of the `stage` step (`:140-141`).
- The 4.4 data-flow diagram (done in this spec).

## 7. Risks and rollback

- The check is weaker: an edge flip between locked versions that `cargo update` made passes. The
  PR body lists every flip. With 4.5, run-2 code cannot make such a flip.
- The stage compare (4.5) runs on a runner only when the family moves. M5 proves it once on a
  scratch branch, positive and negative. The next real proof comes with the next wasm-bindgen
  release.
- If run 2 rewrites the lock in a benign way (wasm-pack's lock repair), `same` refuses every
  bump. M5 measures this before the merge. A later change of wasm-pack can start it. The
  runbook entry for `R-RUN2` applies.
- A future cargo can write a ref in another form. That ref then is not in the canonical ref table
  and gives `R-EDGE`. This fails closed, and the runbook in `rs/CLAUDE.md` applies.
- Open question (not measured): which cargo version wrote the edges on `main`. If Dependabot's
  cargo writes `0.61.2` again, the flips come back every week. The `edge-moved` lines on exit 4
  make this visible in the log.
- Rollback: revert the PR. The weekly run then refuses with `R-NONFAMILY` again, which is the
  known state before this change.
- Targeted rollback of 4.5 only: 4.5 lands in its own commit (the workflow, `pin_check.py`, and
  the `same` subcommand together). A revert of that one commit keeps 4.1 to 4.4, so the weekly
  run keeps its exit 4. The workflow and `pin_check.py` stay consistent, because they change in
  the same commit.
