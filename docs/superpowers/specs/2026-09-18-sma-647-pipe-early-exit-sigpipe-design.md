# SMA-647 — false reds from a pipe into an early-exit reader under `pipefail`

Status: revised after the spec challenge, for GATE 1. Linear: SMA-647.

## 1. Problem

`repo:actionlint` red three times in CI with a false check 12 result. Each time the
literal `.moon/cache/states/` was involved. A re-run of the same SHA passed each time.

- Occurrence 1 (PR 223) and occurrence 3 (PR 258) came from **check 9's unmutated
  control**, which runs `--self-test` in 15 concurrent processes. Both hit the same fixture
  cell: `stderr.log` deleted, with `.moon/cache/states/` as a spurious EXTRA row.
- Occurrence 2 (PR 255) came from **check 12's production call** on the real CLAUDE.md
  block, with `.moon/cache/states/` as a spurious MISSING row.

The site is `ci/actionlint/run.sh:4777`, inside `claude_md_block_verdict`:

```bash
printf '%s' "$block" | grep -qF -- "$lit" || echo "missing-literal $lit"
```

`run.sh` runs under `set -uo pipefail` (line 23).

## 2. Mechanism (MEASURED)

Measurements taken on 2026-09-18 in `ubuntu:24.04` (bash 5.2.21, GNU grep 3.11) under
Docker with 8 busy-loop processes as CPU load, on macOS `/bin/bash` 3.2.57, and on Homebrew
bash 5.3.15.

1. **bash sends the block out in many small writes.** `strace -e trace=write` on
   `printf %s "$block" | cat`, with the real 42-line, 2876-byte CLAUDE.md block, shows 25
   `write(1, …)` calls from the `printf` subshell, of 76 to 191 bytes each. So a write can
   hold more than one line, but the block never goes out in one write.
2. **`grep -q` exits at the first match.** It does not read the rest of the input.
3. **A later `write()` then gets SIGPIPE.** The `printf` subshell exits with 141.
4. **`pipefail` makes the pipeline status 141.** The `|| echo "missing-literal …"` arm fires
   although `grep` found the literal and exited 0.

Reproduction counts (false misses, `PIPESTATUS` in brackets):

| Input | Load | Result |
|---|---|---|
| real block, 5 literals × 3000 runs, bash 5.2 | none | 0 misses |
| real block, 5 literals × 3000 runs, bash 5.2 | 8 CPU hogs | 2 misses (`141/0`), on `stderr.log` and `lastRunTime` |
| self-test fixture (4 lines, 130 bytes), 4 literals × 20000 runs, bash 5.2 | 8 CPU hogs | 2 misses (`141/0`), on `operations[]` |
| deterministic producer: `{ printf 'hit\n'; sleep 0.3; printf 'more\n'; } \| grep -q hit` | none | rc 141 every time, on bash 3.2, 5.2 and 5.3 |

The race needs the reader to exit between two writes of the producer. CPU load makes this
more likely. Check 9's control makes its own load (15 concurrent self-test processes), which
agrees with occurrences 1 and 3. Two of the three failures also came right after
`Creating virtual environment at: ci/release-plan/.venv`.

The mechanism explains BOTH directions: a spurious missing row on the real block, and a
spurious extra row in the fixture (a literal that IS present is reported beside the one that
was deleted).

**What is not explained.** All three CI failures named `.moon/cache/states/`, and
occurrences 1 and 3 hit the SAME fixture cell. In the measurements the false misses fell on
other literals. Under this mechanism alone, two flakes on the same one of about 19
(fixture, literal) cells has a probability of about 5% or less, and occurrence 2 makes it
lower. So a second mechanism is possible. §5.6 keeps an instrument for it, and §6 D5 keeps
the issue open for an observation window.

**What was ruled out as the cause.** The relative `CLAUDE.md` path at `run.sh:6003` (the
issue's first lead). A lost working directory gives the `no-file` verdict, not one missing
literal.

## 3. The same defect elsewhere

The hazard needs three things together:

- (a) a producer that can write more than once after the matching data (in practice, any
  multi-line input);
- (b) a reader that exits early: `grep -q`, `grep --quiet`, `grep -m N`, `grep --max-count`,
  `head`, or `awk` with `exit`;
- (c) a shell where the pipeline status is used and `pipefail` is on.

A pipe into an early-exit reader in a `pipefail` shell is the defect class. The tables below
list every site of the class in the corpus of §5.2 (line numbers at `8bbf2f4c`). The plan
must re-derive this list with the exact search commands it records, before any rewrite.

Sites that can give a false result today:

| File:line | Producer | Effect of a false 141 |
|---|---|---|
| `ci/actionlint/run.sh:4777` | CLAUDE.md block | false `missing-literal` — fails closed (this issue) |
| `ci/actionlint/run.sh:1142` | `$ORIGIN_REFS` list | a real branch reads as unresolved — fails closed |
| `ci/actionlint/run.sh:5618` | `$out` of a sub-run | false self-test failure — fails closed |
| `ci/release-plan/run.sh:83` | multi-line `$out` | runtime fail-safe arm: an unnecessary build and a `::warning::` |
| `ci/release-plan/run.sh:187, 202, 341` | multi-line `$out` | false negative-control failure — fails closed |
| `ci/workflow-credentials/run.sh:93` | `bash "$0"` output | the `if` body is the failure branch, so a false 141 fails OPEN |
| `ci/workflow-credentials/run.sh:97` | `bash "$0"` output | fails closed |
| `ci/publish-metadata/run.sh:1500` | `pypi_scan_paths` | false scan result |
| `ci/release-parity/ecosystems/semantic-release.sh:35` | commit footer (multi-line) | false "no breaking change" |
| `ops/nats/check-subjects.sh:101, 108` | `$pub_declared` list | false "UNDECLARED grant" — fails closed; runs in `repo:nats-permissions` |
| `.github/workflows/wheels.yml:521` | `tar tzf` listing | the `if` body is the failure branch, so a false 141 fails OPEN |
| `.github/workflows/wheels.yml:544, 677` | listing, wheel `METADATA` | fails closed |
| `.github/workflows/wheels.yml:312, 323`, `.github/workflows/prebuild.yml:187` | `otool -l` output into `awk '…{print $2; exit}'` | the step stops with no `::error::` line (the `release.yml:913` shape) |

Sites of the class that are safe today, for the stated reason. They are still rewritten
(D3):

| File:line | Why safe today |
|---|---|
| `ci/actionlint/run.sh:1066`, `ci/release-parity/ecosystems/semantic-release.sh:34`, `.lefthook/pre-push/check-branch.sh:22` | one short token with no newline |
| `.github/workflows/ci.yml:271` and its copies at `ci/actionlint/run.sh:4273-4408` | one sha with no newline |
| `ci/ruff/run.sh:148` | the producer ends in `sort` (`ci/ruff/run.sh:62`), which is block-buffered and writes once |
| `ci/actionlint/run.sh:1173, 2482, 2492, 2506, 2519, 2565, 2578` | inside `$( )` with the status discarded, and `run.sh` has no `set -e` |
| `ci/images/run.sh:49-117`, `ci/publish-metadata/run.sh:536` | `grep` output to `head` is block-buffered and small; 536 also has `|| true` |
| `moon.yml:373` | the Moon script has no `pipefail`, so the status is `grep`'s |

`release.yml:913` and `prebuild.yml:266` already record this hazard for `sort -V | head -n1`
(SMA-602, fix round 4). That fix was local to one step, and nothing stopped the pattern
from coming back elsewhere. This spec is the general fix.

## 4. Goals and non-goals

Goals:

- G1. No site in the corpus of §5.2 pipes a producer into an early-exit reader.
- G2. A new such site reds `repo:actionlint`.
- G3. The mechanism is recorded once, in CLAUDE.md's Gotchas, so it is not diagnosed again.

Non-goals:

- Anchoring the `CLAUDE.md` path at `run.sh:6003`. It is not the cause (§2).
- Printing the block's size and hash on a miss (the issue's second suggestion). §5.6's
  second-opinion row is a sharper instrument for the one open question.
- Python `subprocess` pipes, and scripts in `ts/` and `py/` that are not `*.sh`.
- `grep -l`, `grep -L` and `--files-with-matches` on stdin. GNU grep reads a pipe to EOF in
  these modes, BSD grep may not, and the tree has no such site.

## 5. Design

### 5.1 The replacement idioms

The rule: **the pipeline status must never depend on a producer that an early-exit reader
can kill.** Two idioms, chosen per site:

- **I1 — process substitution**, when the producer's own exit status does not matter:

  ```bash
  grep -qF -- "$lit" < <(printf '%s' "$block")
  ```

  The producer is not part of the pipeline, so `pipefail` never sees its 141. `grep`'s
  semantics are unchanged. MEASURED: 0 false misses under load, and rc 0 on the
  deterministic slow-producer test, on bash 3.2.57, 5.2.21 and 5.3.15.

  I1 is valid in every context of the corpus: GitHub Actions runs `run:` blocks under bash
  (ubuntu default or explicit `shell: bash`), Moon's default script shell is bash with no
  `unixShell` override in this repo, check 8d executes the `ci.yml` block with
  `"$bash_bin" -c`, and the sourced ecosystem modules run under bash.

  A here-string (`<<<`) is NOT used: Homebrew bash 5.3.15 deadlocks on a here-string over
  about 512 bytes (CLAUDE.md Gotchas). `>/dev/null` in place of `-q` is NOT used: it depends
  on how a given grep build treats `/dev/null` (it happened to read to EOF on GNU grep 3.11).

- **I2 — capture, then match**, when the producer's exit status DOES matter: run the
  producer into a variable, check its rc explicitly, then match the variable with I1.

  Pitfalls the plan must avoid: `local v="$(…)"` hides the rc (`local` returns 0); declare
  first, then assign. A bare `v="$(…)"` under `set -e` stops the script with the producer's
  code before the explicit check can run; use `if ! v="$(…)"; then …` (the shape at
  `moon.yml:369-372`).

  For each site, the plan must state whether the old pipeline's `pipefail` was also catching
  a real producer failure. If it was, I2 must keep that failure visible. A rewrite that turns
  a producer failure into a silent pass is a regression. The two fail-OPEN sites in §3
  (`workflow-credentials/run.sh:93`, `wheels.yml:521`) need this analysis most.

- **Value readers.** `head -1` and `grep -m1 … | sed` are replaced by a reader that consumes
  all of its input, for example `sed -n 1p`. `awk '…{print $2; exit}'` becomes
  `awk '…{v=$2} END{print v}'` style, or the first-match equivalent without `exit`. A `sed`
  script with `q` is NOT allowed. The plan picks the exact form per site and keeps the old
  first-match semantics.

### 5.2 The ban — a new check 13 in `ci/actionlint/run.sh`

**Corpus.** `git ls-files` with these exact pathspecs:

```
':(glob)**/*.sh'
':(glob).github/workflows/*.yml'  ':(glob).github/workflows/*.yaml'
'moon.yml'  ':(glob)**/moon.yml'
':(glob).moon/**/*.yml'
'lefthook.yml'
```

`:(glob)` is required. Without it, git's `**` still needs a literal `/`, so `.moon/**/*.yml`
misses `.moon/tasks.yml`, and `**/moon.yml` misses the root `moon.yml` (the trap recorded for
`repo:ruff-ci`). The root `moon.yml` is listed on its own for the same reason.

**Corpus floor.** An empty or partial corpus passes with no assertion, as check 12 records
(`run.sh:5898-5902`). So check 13 asserts that named members are present:
`ci/actionlint/run.sh`, `.github/workflows/ci.yml`, `moon.yml`, `.moon/tasks.yml`,
`ops/nats/check-subjects.sh`. A missing member, or a corpus that cannot be listed, is rc 2
(`infra`). The floor lines are pinned (§5.4).

**Rule.** Line joining first: a line that ends in `|` or `|\` (after trailing spaces) is
joined with the next line, because bash accepts a bare `|` at the end of a line. Then a
full-line comment (first non-blank character `#`) is skipped. Then the line matches if a
pipe `[|]` that is not part of `||` or `|&` is followed by optional spaces, optional
`VAR=value` assignments, an optional `command ` prefix, and then one of:

- `grep` with a flag word that contains `q` or `m` in a short-flag cluster (`-q`, `-qF`,
  `-nxm1`), or the long flag `--quiet`, `--silent` or `--max-count`, before the next `|`;
- `head` as a whole word;
- `awk` with the word `exit` before the next `|`.

The rule matches the NEXT COMMAND WORD after the pipe, not any later text. So
`| xargs grep -q` does not match (xargs reads to EOF), and `grep -E 'a|head'` does not match.
The ERE uses POSIX classes only (no `\s` or `\b`), because `run.sh:2529-2531` records BSD and
GNU differences. The plan writes the ERE out in full, and the rule's own definition line uses
bracket expressions (`[|]`) so that it does not match itself.

**Verdict rows.** `early-exit-reader <path>:<line>`, sorted with `LC_ALL=C`. Any row reds the
gate at rc 1.

**Allowlist.** `EARLY_EXIT_READER_ALLOWED`, keyed by path, line number AND the line's own
text (the `COE_SKIP` shape, so a shifted entry stops matching instead of absorbing another
identical line), with a required reason. A row with no reason reds the gate. A row that no
longer matches a line is a NOTE, not a red, as check 12 treats a stale row
(`run.sh:5972-5973`). It ships EMPTY (D3). Every expansion of the table uses the
`${ARR+"${ARR[@]}"}` idiom (`run.sh:1115`), because bash 3.2 under `set -u` cannot expand an
empty array: the error kills the verdict's process substitution, no rows come out, and a real
violation passes in silence (`run.sh:5946-5955`).

**Fixtures live outside the corpus.** Check 13 scans `ci/actionlint/run.sh`, so any fixture
line written inside `run.sh` would fire. The fixtures are therefore data files under
`ci/actionlint/fixtures/early-exit/` with a `.txt` extension, which no corpus pathspec
matches. Fail messages in `run.sh` do not quote a banned form literally. This is option (a)
of the challenge; the alternatives were to build fixtures from fragments (fragile, which
`run.sh:4677-4679` warns about) or to allow line-keyed rows for the gate's own text (which
breaks D3).

**Self-test.** `early_exit_reader_self_test`, fixture-driven. It must cover at least: each
reader form fires (`-q`, a combined cluster, `--quiet`, `-m1`, `--max-count`, `head`,
`awk … exit`); `VAR=x grep -q` and `command grep -q` fire; a pipe at the end of a line joined
with a `grep -q` line fires; `||` does not fire; `|&` into a non-reader does not fire;
`| xargs grep -q` does not fire; `grep -E 'a|head'` does not fire; a full-line comment does
not fire; I1 and I2 lines do not fire; an allowlisted line with a reason is silent; a
blank-reason row fires; a stale row is a note; an EMPTY table with one real hit still emits
the row (the bash 3.2 case). `SELF_TEST_COUNT` goes from 14 to 15.

### 5.3 Proof that the fix bites (behavioural)

`early_exit_reader_self_test` owns one behavioural case that uses a HANDSHAKE, not a sleep:

- The reader is `{ grep -q hit; exec <&-; : > "$flag"; }`. It closes its own stdin after
  `grep` returns, then touches a flag file.
- The producer writes `hit`, waits for the flag in a bounded loop (at most 10 s), then writes
  again.
- Old pipe form: assert `PIPESTATUS[0] != 0` and `PIPESTATUS[1] == 0`. This does not assume
  exactly 141, because with SIGPIPE ignored the producer fails with EPIPE and rc 1.
- I1 form, with the same producer and the same reader: assert rc 0.

The first assertion proves the case can see the defect. Without it, a case that never races
proves nothing. The handshake removes the timing dependency: the second write happens only
after the reader has closed the pipe. This matters because `run_self_tests` runs 16 times per
gate run (once directly, and in check 9's 15 concurrent control processes), which is the same
load that produced occurrences 1 and 3. If the flag does not appear within the bound, the
case reports `infra`, not a verdict.

### 5.4 Registries and pins that move

Each rewritten line that is pinned elsewhere must have its pin updated in the same commit, or
`repo:affected-smoke` reds. The plan searches every pin table for each rewritten line's old
text. Known pins:

- `ci/affected-graph/ci_targets.py:105` (the `ci.yml:271` `BEFORE` line), `:1088`
  (`workflow-credentials` line 93), `:1140`, `:1146` (`release-plan` lines 83 and 341),
  `:2522`.
- The copies of the `ci.yml` step embedded in `ci/actionlint/run.sh:4273-4408` (check 8d's
  mutation battery), which must stay byte-faithful to `ci.yml`.
- `WORKFLOW_CREDENTIALS_SH_CALL_SITES` (`ci_targets.py:1079-1091`) pins
  `workflow-credentials/run.sh:93` as its ASSERTION line. If I2 splits that line into a
  capture line and a match line, both lines are pinned, because deleting either one must red.

Check 13 is a new check inside an existing gate, not a new `repo:*` task. So it needs no `T`
array entry, no CLAUDE.md marker-command change, and no `SELF_SCHEDULED_GATES` entry. It
DOES need, all firm:

- the `SELF_TEST_COUNT` bump, the `run_self_tests` call, and the definition-count assertion;
- `ACTIONLINT_SH_CALL_SITES` (`ci_targets.py:910`, `:916-945`) pins check 13's production
  `done < <(…)` line and its corpus-floor lines, whole-line and at column 0, as it pins
  check 12's. `ci_targets.py:938-944` records that without such a pin, deleting check 12's
  read loop left every gate green;
- the same lines added to the hand-maintained `wired_actionlint` fixture
  (`ci_targets.py:2433-2503`), with a deletion case and an indentation case, as
  `no_check8d_call` (`ci_targets.py:2680`) does.

### 5.5 Documentation

- A new CLAUDE.md Gotchas entry: the mechanism, the idioms, the here-string exclusion, and
  that check 13 enforces it. It names SMA-647. It must not contain the strings
  `moon-diagnosis:begin` or `moon-diagnosis:end`, because check 12 counts them anywhere in the
  file.
- Every "fourteen" / "14" self-test count: `run.sh:48-51` (`SELF_TEST_COUNT` comment),
  `run.sh:66-70` (usage text), `run.sh:4927` ("All FOURTEEN"), `ci/actionlint/README.md:45`
  and `:715-760`, and CLAUDE.md's "currently 14" sentence.
- CLAUDE.md's `workflow-credentials` entry ("the fifth is an ASSERTION line") if §5.4 splits
  line 93.
- `ci/actionlint/README.md`'s check table gets a check 13 row, and its Limitations get §7.
- `release.yml:913` and `prebuild.yml:266` comments gain one sentence that points to check 13.
- The plan document itself: if it quotes `ciReport.json`, it carries
  `<!-- moon-diagnosis:ok -->` (check 12).
- The Linear issue gets the mechanism and the measurements as a comment.

### 5.6 Second opinion on check 12's miss path

In `claude_md_block_verdict`, when the I1 `grep` reports a literal missing, the function tests
again with a pure-bash match, `case "$block" in *"$lit"*)`. If the two results disagree, it
emits a separate row, `literal-disagreement <literal>`, which reds the gate with a message
that names SMA-647. After the fix, I1 cannot race, so a disagreement row would be evidence of
a second mechanism (§2, "What is not explained"). The self-test covers the agreeing miss path;
the disagreeing path cannot be produced by a fixture without a fake `grep`, and the plan
decides whether a PATH-stubbed `grep` fixture is worth it.

## 6. Decisions

- **D1 — fix the class, not only check 12.** Sven chose this at intake (2026-09-18). One
  local fix of this class already exists (`release.yml:913`, SMA-602), and the class came
  back elsewhere.
- **D2 — process substitution over a pure-bash `case` match.** `case` would fix line 4777,
  but it does not cover the `-x`, `-E` or `-w` semantics at the other sites. One idiom for all
  sites is easier to review and to ban against.
- **D3 — rewrite the safe sites too, and ship the allowlist empty.** The alternative is about
  15 reasoned rows, each a claim about buffering that a later edit can falsify in silence (a
  one-token producer becomes multi-line). This includes `ci.yml:271`, the most-pinned line in
  the repository (7 copies): the rewrite is one mechanical text substitution applied to every
  copy, and a text-keyed allowlist row would stop matching after any later edit to that line
  anyway.
- **D4 — static ban, not a runtime detector.** A runtime detector cannot see a race that did
  not happen in that run.
- **D5 — the PR references SMA-647 but the issue stays open for an observation window** of
  two weeks of CI on `main` after the merge. The issue closes if no `literal-disagreement` row
  and no check 12 or check 9 flake appears in that window. A PR that names a Linear key
  auto-links by branch name; the close is a manual step.
- **D6 — one PR.** The challenge suggested two (the real hazard sites first, then check 13 and
  the safe-site rewrites). One PR is chosen, because check 13 cannot turn on until every site
  is rewritten, and a split leaves the ban unenforced between the two merges.

## 7. Limitations

- L1. Early-exit readers outside the rule's vocabulary (`sed q`, `read`, `perl … last`) are
  not seen.
- L2. The rule reads the next command word only. A reader reached through a wrapper that is
  not in the vocabulary (`| timeout 5 grep -q`, `| env grep -q`) is not seen.
- L3. Surfaces outside the corpus are not scanned (§4 non-goals). In particular a `bash -c`
  string inside a `.py` file is not seen.
- L4. The rule is textual. A `grep -q` inside a quoted string or a heredoc body in a `.sh`
  file fires. This is the reason the fixtures live in `.txt` files (§5.2).
- L5. `repo:actionlint` still has no working local bash (CLAUDE.md). The self-tests can run
  locally with `bash ci/actionlint/run.sh --self-test` under the bash the plan verifies; the
  whole gate is verified only in CI.

## 8. Testing

- `early_exit_reader_self_test` (§5.2) and the behavioural handshake case (§5.3).
- A mutation run: re-introduce the old line 4777 and confirm check 13 reds with exactly one
  row naming it; revert by deleting the inserted line, not with `git checkout` (which would
  also revert the uncommitted fix).
- Every rewritten gate's own `--self-test` / `--negative-control` / real run passes, with the
  bash each gate needs (CLAUDE.md: `affected-smoke` 3.2; `ruff-ci`, `next-public-free`,
  `publish-metadata` 4+).
- The full CI graph on the PR. `wheels.yml` and `prebuild.yml` have path-filtered
  `pull_request` triggers that include the workflow files themselves, so the PR runs them.
  `images.yml` is not a required check: the PR author watches it after the `ci/images/run.sh`
  rewrite. `release-plan/run.sh:83` runs for real only at release time, so only its
  negative-control rows cover it on the PR.
