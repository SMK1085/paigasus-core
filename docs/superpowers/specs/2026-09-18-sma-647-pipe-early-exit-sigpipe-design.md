# SMA-647 — false reds from a pipe into an early-exit reader under `pipefail`

Status: draft for GATE 1. Linear: SMA-647.

## 1. Problem

`repo:actionlint` red three times in CI with a false check 12 result. Each time the
literal `.moon/cache/states/` was involved. A re-run of the same SHA passed each time.

The site is `ci/actionlint/run.sh:4777`, inside `claude_md_block_verdict`:

```bash
printf '%s' "$block" | grep -qF -- "$lit" || echo "missing-literal $lit"
```

`run.sh` runs under `set -uo pipefail` (line 23).

## 2. Mechanism (MEASURED)

All measurements below were taken on 2026-09-18 in `ubuntu:24.04` (bash 5.2.21, GNU grep
3.11) under Docker, with 8 busy-loop processes as CPU load, and on macOS `/bin/bash` 3.2.57.

1. **bash writes a builtin's stdout one line per `write()`.** `strace -e trace=write` on
   `printf %s "$block" | cat`, with the real 42-line, 2876-byte CLAUDE.md block, shows 25
   separate `write(1, …)` calls from the `printf` subshell, one per line (bash line-buffers
   its stdout).
2. **`grep -q` exits at the first match.** It does not read the rest of the input.
3. **A later `write()` then gets SIGPIPE.** The `printf` subshell exits with 141.
4. **`pipefail` makes the pipeline status 141.** The `|| echo "missing-literal …"` arm fires
   although `grep` found the literal and exited 0.

Reproduction counts (false misses, `PIPESTATUS` in brackets):

| Input | Load | Result |
|---|---|---|
| real block, 5 literals × 3000 runs | none | 0 misses |
| real block, 5 literals × 3000 runs | 8 CPU hogs | 2 misses (`141/0`), on `stderr.log` and `lastRunTime` |
| self-test fixture (4 lines, 130 bytes), 4 literals × 20000 runs | 8 CPU hogs | 2 misses (`141/0`), on `operations[]` |
| deterministic producer: `{ printf 'hit\n'; sleep 0.3; printf 'more\n'; } \| grep -q hit` | none | rc 141 every time, both bash versions |

So the race needs the reader to exit between two writes of the producer. CPU load makes
this more likely. In CI, two of the three failures came right after
`Creating virtual environment at: ci/release-plan/.venv`, which is concurrent work on the
same runner. That agrees with the mechanism, but it does not prove the cause of those two
runs.

The mechanism explains BOTH directions seen in the issue:

- **Spurious missing row** (occurrence 2): the real block, the production call.
- **Spurious extra row** (occurrences 1 and 3): the self-test's `miss.md` fixture. There a
  literal that IS present is also reported, beside the one that was deleted.

**What is not explained.** All three CI failures named `.moon/cache/states/`. In the
measurements the false misses fell on other literals. The mechanism does not depend on the
literal, and three samples are few, so this can be chance. The spec does not claim to
explain the literal's identity.

**What was ruled out as the cause.** The relative `CLAUDE.md` path at `run.sh:6003` (the
issue's first lead). A lost working directory gives the `no-file` verdict, not one missing
literal, and the measured mechanism accounts for all three failures without it.

## 3. The same defect elsewhere

The hazard needs three things together:

- (a) a producer that can write more than once after the matching line (in practice, any
  multi-line input, since bash writes one line per `write()`);
- (b) a reader that exits early: `grep -q`, `grep --quiet`, `grep -m N`, `grep --max-count`,
  or `head`;
- (c) a shell where the pipeline status is used and `pipefail` is on.

A search of the tracked tree finds these sites with all three (line numbers at `8bbf2f4c`):

| File:line | Producer | Result of a false result |
|---|---|---|
| `ci/actionlint/run.sh:4777` | CLAUDE.md block | false `missing-literal` (this issue) |
| `ci/actionlint/run.sh:1142` | `$ORIGIN_REFS` list | a real branch reads as unresolved |
| `ci/actionlint/run.sh:5618` | `$out` of a sub-run | false self-test failure |
| `ci/release-plan/run.sh:83, 187, 202, 341` | multi-line `$out` | false negative-control failure |
| `ci/workflow-credentials/run.sh:93, 97` | `bash "$0"` output | false control verdict |
| `ci/ruff/run.sh:148` | `ruff_corpus` | false corpus miss |
| `ci/publish-metadata/run.sh:1500` | `pypi_scan_paths` | false scan result |
| `ci/release-parity/ecosystems/semantic-release.sh:35` | commit footer (multi-line) | false "no breaking change" |
| `.github/workflows/wheels.yml:521, 544` | `tar tzf` listing | false sdist content verdict |
| `.github/workflows/wheels.yml:677` | wheel `METADATA` | false dependency verdict |

These sites have (b) but are safe today for a stated reason. They are still rewritten, so
that the ban in §5 needs no allowlist (see D3):

| File:line | Why safe today |
|---|---|
| `ci/actionlint/run.sh:1066`, `.github/workflows/ci.yml:271` and its copies at `ci/actionlint/run.sh:4273-4408` | one token with no newline: one `write()` |
| `ci/actionlint/run.sh:1173, 2482, 2492, 2506, 2519, 2565, 2578` | inside `$( )` with the status discarded, and `run.sh` has no `set -e` |
| `ci/images/run.sh:49-117`, `ci/publish-metadata/run.sh:536` | `grep` output to `head` is block-buffered and small; 536 also has `|| true` |
| `moon.yml:373` | the Moon script has no `pipefail`, so the status is `grep`'s |

`release.yml:913` and `prebuild.yml:266` already record this hazard for `sort -V | head -n1`
(SMA-602, fix round 4). That fix was local to one step, and nothing stopped the pattern
from coming back elsewhere. This spec is the general fix.

## 4. Goals and non-goals

Goals:

- G1. No site in the scanned surfaces (§5) pipes a producer into an early-exit reader.
- G2. A new such site reds `repo:actionlint`.
- G3. The mechanism is recorded once, in CLAUDE.md's Gotchas, so it is not diagnosed again.

Non-goals:

- Explaining why CI named `.moon/cache/states/` each time (§2).
- The issue's second suggestion (print the block's size and hash on a miss). With the
  mechanism known, it adds no diagnosis. Not done.
- Anchoring the `CLAUDE.md` path at `run.sh:6003`. It is not the cause (§2). Not done.
- Scripts outside the scanned surfaces (Python `subprocess` pipes, `ts/` or `py/` scripts).

## 5. Design

### 5.1 The replacement idioms

The rule: **the pipeline status must never depend on a producer that an early-exit reader can
kill.** Two idioms, chosen per site:

- **I1 — process substitution**, when the producer's own exit status does not matter:

  ```bash
  grep -qF -- "$lit" < <(printf '%s' "$block")
  ```

  The producer is not part of the pipeline, so `pipefail` never sees its 141. `grep`'s
  semantics are unchanged. MEASURED: 0 false misses in 20000 runs under load, and rc 0 on the
  deterministic slow-producer test, on bash 5.2 and 3.2.

  A here-string (`<<<`) is NOT used. On this class of macOS machine, Homebrew bash 5.3.15
  deadlocks on a here-string over about 512 bytes (CLAUDE.md Gotchas), and the real block is
  2876 bytes.

- **I2 — capture, then match**, when the producer's exit status DOES matter (for example
  `bash "$0"` in `workflow-credentials`, `ruff_corpus`, `pypi_scan_paths`): run the producer
  into a variable, check its rc explicitly, then match the variable with I1. `moon.yml:373`
  is the existing example of this shape.

  For each site, the plan must state whether the old pipeline's `pipefail` was also catching
  a real producer failure. If it was, I2 must keep that failure visible. A rewrite that turns
  a producer failure into a silent pass is a regression.

For a reader that returns a VALUE (`head -1`, `grep -m1 … | sed`), the rewrite uses a reader
that consumes all of its input, for example `sed -n 1p`. A `sed` script with `q` is NOT
allowed, because `q` exits early. The plan picks the exact form per site.

`>/dev/null` in place of `-q` is NOT an allowed idiom. It depends on how a given grep build
treats `/dev/null`. It happened to read to EOF on GNU grep 3.11, but the gate must not rest on
that.

### 5.2 The ban — a new check 13 in `ci/actionlint/run.sh`

A static scan of tracked files, same shape as check 12:

- **Corpus:** `git ls-files` over `ci/**/*.sh` (with the `ci/*.sh` companion pathspec, for the
  git `**` trap recorded for `repo:ruff-ci`), `.github/workflows/*.yml`, and every tracked
  `moon.yml` plus `.moon/**/*.yml`.
- **Rule:** a non-comment line that contains a `|` followed, on the same line, by `grep` with
  `-q`, `--quiet`, `-m`, `--max-count` (in any combined short-flag cluster, for example
  `-qF`, `-nxm1`), or by `head`. A `||` is not a pipe and must not match.
- **Verdict rows:** `early-exit-reader <path>:<line>`. Any row reds the gate at rc 1. A
  corpus that cannot be listed is rc 2 (`infra`), as check 12 does.
- **Allowlist:** `EARLY_EXIT_READER_ALLOWED`, keyed by path AND the line's own text (the
  `COE_SKIP` shape, so a shifted entry stops matching instead of absorbing another line), with
  a required reason. A row with no reason, and a row that no longer matches a line, are both
  reported. It ships EMPTY (D3).
- **Self-test:** `early_exit_reader_self_test`, fixture-driven like
  `doc_diagnosis_self_test`. It must cover at least: each reader form fires; a combined flag
  cluster fires; `||` does not fire; a commented line does not fire; I1 and I2 lines do not
  fire; an allowlisted line with a reason is silent; a blank-reason row fires; a stale row
  fires. `SELF_TEST_COUNT` goes from 14 to 15.

### 5.3 Proof that the fix bites

Two proofs, both required:

- **Static:** the check-13 self-test above.
- **Behavioural:** one fixture case in `doc_diagnosis_self_test` (or in the new self-test)
  that drives the I1 idiom with the deterministic slow producer
  (`{ printf 'hit\n'; sleep 0.3; printf 'more\n'; }`) and asserts rc 0, and drives the old
  pipe form and asserts rc 141. The second assertion proves the fixture can see the defect at
  all; without it, a fixture that never races proves nothing. The sleep makes it
  deterministic, not probabilistic.

### 5.4 Registries and pins that move

Several rewritten lines are pinned as whole lines elsewhere. The plan must update each pin in
the same commit as the line it pins, or `repo:affected-smoke` reds:

- `ci/affected-graph/ci_targets.py:105` (the `ci.yml:271` `BEFORE` line), `:1088`
  (`workflow-credentials` line 93), `:1140`, `:1146` (`release-plan` lines 83 and 341),
  `:2522`.
- The copies of the `ci.yml` step embedded in `ci/actionlint/run.sh:4273-4408` (check 8d's
  mutation battery), which must stay byte-faithful to `ci.yml`.
- Any `*_SH_CALL_SITES` table that names a rewritten line. The plan must search every pin
  table for each rewritten line's old text.

Check 13 itself is a new check inside an existing gate, not a new `repo:*` task. So it needs
no `T` array entry, no CLAUDE.md marker-command change, and no `SELF_SCHEDULED_GATES` entry.
It DOES need: the `SELF_TEST_COUNT` bump, the `run_self_tests` call, the definition-count
assertion, and a row in `ci/actionlint/README.md`'s check table. If `ci_targets.py`'s
`ACTIONLINT_SH_CALL_SITES` pins the check-12 production call, check 13's production call gets
the same pin.

### 5.5 Documentation

- A new CLAUDE.md Gotchas entry: the mechanism, the two idioms, the here-string exclusion, and
  that check 13 enforces it. It names SMA-647.
- `release.yml:913`'s comment gains one sentence that points to check 13.
- The Linear issue gets the mechanism and the measurements as a comment.

## 6. Decisions

- **D1 — fix the class, not only check 12.** Sven chose this at intake (2026-09-18). One bug
  fixed in one place already came back once (`release.yml:913`, SMA-602).
- **D2 — process substitution over a pure-bash `case` match.** `case "$block" in *"$lit"*)`
  would also fix line 4777, but it does not cover `-x`, `-E` or `-w` semantics at the other
  sites. One idiom for all sites is easier to review and to ban against.
- **D3 — rewrite the safe sites too, and ship the allowlist empty.** The alternative is about
  15 reasoned allowlist rows, each a claim about buffering that a later edit can falsify in
  silence (a one-token producer becomes multi-line). A rewrite has no such claim to rot.
- **D4 — static ban, not a runtime detector.** A runtime detector cannot see a race that did
  not happen in that run.

## 7. Limitations

- L1. The scan is per physical line. A pipe split over a continuation (`producer |\` then
  `grep -q` on the next line) is not seen. The corpus has no such line today; the plan must
  confirm that with a multi-line search.
- L2. Early-exit readers outside the rule's vocabulary (`awk '… exit'`, `sed q`, `read`) are
  not seen.
- L3. Surfaces outside §5.2's corpus are not scanned (§4 non-goals).
- L4. `repo:actionlint` still has no working local bash (CLAUDE.md). Check 13's self-test can
  be run locally by sourcing its functions, but the whole gate is verified only in CI.

## 8. Testing

- Check 13's self-test (§5.2) and the behavioural proof (§5.3).
- A mutation run: re-introduce the old line 4777 and confirm check 13 reds with exactly one
  row naming it; revert.
- Every rewritten gate's own `--self-test` / `--negative-control` / real run passes, with the
  bash each gate needs (CLAUDE.md: `affected-smoke` 3.2; `ruff-ci`, `next-public-free`,
  `publish-metadata` 4+).
- The full CI graph on the PR. `wheels.yml` has a path-filtered `pull_request` trigger that
  includes the workflow file itself, so the PR runs it.
