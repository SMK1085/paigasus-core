# SMA-612 — `repo:actionlint` hangs on macOS: fail loud on a small pipe

- **Linear:** [SMA-612](https://linear.app/smaschek/issue/SMA-612)
- **Status:** design, revised after the adversarial challenge, awaiting approval
- **Approach chosen by Sven:** "Fail loud + bump later" (option 1 of 4, 2026-09-19).

## 1. Problem

On the development Mac, `bash ci/actionlint/run.sh` runs for many minutes with no output. The
issue recorded 560 seconds, then a kill. CI (Linux) passes. The failure is a silent hang, so a
person reads it as "a slow gate", not as "a broken gate".

## 2. Root cause (measured on 2026-09-19)

Host: macOS 26.6.2, Darwin 25.6.0, arm64. actionlint 1.7.12 (proto pin). shellcheck 0.11.0 from
`py/.venv` (the gate's own pin). All measurements ran in a Claude Code agent session: M1–M9 in
its Bash sandbox, M10 with the sandbox disabled. A plain Terminal session is NOT measured (§10).

| # | Measurement | Result |
|---|---|---|
| M1 | `actionlint -pyflakes= -shellcheck=<pin> .github/workflows/ci.yml`, 15 s, then `SIGQUIT` | The main goroutine waits in `RuleShellcheck.VisitWorkflowPost`. Two worker goroutines block in `io.WriteString` at `process.go:32`, writing 870 and 538 bytes. |
| M2 | Process tree during the hang | No shellcheck process exists. actionlint uses about 300% CPU. |
| M3 | `sample` of actionlint during the hang | The hot stacks are `kevent` and `write`: a busy loop. |
| M4 | Fill a new pipe with non-blocking one-byte writes (Python `os.pipe`), five pipes | Each pipe accepts **512 bytes**. |
| M5 | Write 513, 870 and 2000 bytes into an EMPTY new pipe | Each write returns 512 (a partial write). kqueue `EVFILT_WRITE` then reports **15872** (= 16384 − 512) free bytes, but the pipe is full. |
| M6 | Fill, drain and fill the same pipe three times | 512 bytes each time. The pipe does not grow when a reader is present. |
| M7 | Size of every `run:` block in `.github/workflows/*.yml` | 88 blocks. 22 are over 512 bytes, and all 22 are in `ci.yml`, `prebuild.yml`, `release.yml` and `wheels.yml` — the four workflows the issue names. The largest is 2498 bytes (`wheels.yml`, job `build`, step 13). |
| M8 | `/opt/homebrew/bin/bash ci/actionlint/run.sh --self-test`, 60 s | Still running, no output, **0.14 s of CPU**: the bash deadlock, not the actionlint busy loop. |
| M9 | `/opt/homebrew/bin/bash -c 'wc -c <<<"$x"'` for `$x` of N spaces | N = 400, 510, 511 finish (401, 511, 512 bytes with the newline). N = 512, 513, 600, 2000 hang. The boundary is exactly a 512-byte pipe. |
| M10 | M4 and a 600-byte M9, with the Bash sandbox disabled | Capacity 512. The here-string hangs. The sandbox does not cause the limit. |

**Mechanism 1 — actionlint.** actionlint 1.7.12 `cmdExecution.run` calls `cmd.StdinPipe()`,
writes the whole script, and closes the pipe. Only after that does `cmd.Output()` start
shellcheck. So the whole script must fit in the pipe buffer, because no reader exists yet. A
script over the pipe capacity blocks forever. kqueue reports free space that the pipe does not
have (M5), so Go's poller wakes, the write fails with `EAGAIN`, and the loop repeats (M2, M3).

**Mechanism 2 — bash 5.3 here-strings.** Homebrew bash 5.3.15 writes a small here-string or
here-document into a pipe before the reader starts. M9 shows the hang starts exactly above 512
bytes. `ci/actionlint/run.sh` uses here-strings on the self-test path (for example
`run.sh:2446`, `done <<<"$recs"`), so under this bash `run_self_tests` deadlocks (M8) before the
gate ever reaches check 1. `/bin/bash` 3.2.57 uses temporary files for here-strings, so it
reaches check 1 and then hangs in mechanism 1. This is the "here-string deadlock over roughly
512 bytes" that CLAUDE.md and the memory files record. M9 turns that note from a size estimate
into a measured boundary with a known cause.

**Both mechanisms have one trigger: a new pipe holds only 512 bytes.** Both processes use
`pipe(2)`: Go's `os.Pipe` on darwin has no `pipe2`, and CPython's `os.pipe` also calls `pipe(2)`.
So a probe in a separate Python process sees the same limit, if the limit is not per process.
M1, M4 and M9 were taken in three different processes on the same host and agree, which supports
that.

**Upstream.** actionlint issue [#650](https://github.com/rhysd/actionlint/issues/650) reports the
same stacks on Darwin 25.4 with scripts of about 1 KB. Commit `fd33e9f582` (2026-04-17) fixes
it: it sets `cmd.Stdin` to a `strings.Reader`, so Go copies the bytes after the child starts.
`gh release list -R rhysd/actionlint` on 2026-09-19 shows v1.7.12 (2026-03-30) as the latest
release. No release contains the fix. The design did not look for nightly builds; the repo pins
releases only.

**What is not known.** macOS does not expose the pipe kernel-memory counters
(`kern.ipc.maxpipekva` is an unknown OID). A probable model (a hypothesis, from XNU
`sys_pipe.c` as recalled, not re-read): a new pipe starts at 512 bytes and grows only while a
system-wide pipe-memory counter is below its maximum; kqueue assumes it can always grow to 16384
(M5). If that is correct, the 512 state is exhaustion of pipe memory for the whole host, and it
changes when any process opens or closes pipes. The design does not claim that a reboot or fewer
processes removes the state: that is not measured. The upstream reporter saw the same limit, so
it is not unique to this host.

## 3. Decision

Until an actionlint release contains `fd33e9f582`:

1. **D1 — Probe first, in full-gate mode.** Directly after the argument `case`
   (`run.sh:5394-5402`) and BEFORE `run_self_tests`, the gate calls a preflight function when
   `SELF_TEST_ONLY=0`. The preflight measures the capacity of a new pipe. If the capacity is
   below the floor, the gate exits through `infra` (rc 2) and names both mechanisms. It starts no
   fixture table and no actionlint. The probe must run before `run_self_tests`, because under
   the default bash on this host (Homebrew 5.3.15, which Moon's `bash -c` and a bare `bash` both
   resolve first) the self-tests deadlock before any later code runs (M8).
2. **D2 — `--self-test` mode does not probe.** The mutation battery (check 9) starts sixteen
   concurrent `--self-test` subprocesses. A probe there adds a `uv run` to each. The existing
   `SHELLCHECK_BIN` comment (`run.sh:5812-5820`) rejects that cost for the same reason. So
   `--self-test` under Homebrew bash on a small-pipe host still hangs; §9 R2 records it. Under
   `/bin/bash` 3.2 `--self-test` works on such a host (CLAUDE.md, SMA-647 measurement), and a
   probe would block it for no gain.
3. **D3 — Floor = 8192 bytes.** The probe detects "the kernel does not grow a new pipe past the
   512-byte minimum". It does not detect "capacity below normal". Any floor above the largest
   payload and at or below every healthy value detects M4. 8192 is 3.3 times the largest `run:`
   block (M7). It is below the macOS nominal 16384 (M5) and the Linux default 65536. It is also
   at or below the Linux soft-limit fallback of two pages, so a Linux user over
   `fs.pipe-user-pages-soft` gets no false rc 2. The healthy macOS value is not measured on this
   host, because this host never shows it (§10).
4. **D4 — rc 2, never rc 1 or rc 0.** The gate asserted nothing. rc 1 would say "a workflow is
   wrong", which is false. rc 0 would be a false pass. rc 2 is the gate's existing
   "infrastructure error" code (`run.sh:16-18`).
5. **D5 — Fail closed everywhere.** A probe output that is not a valid positive integer gives
   rc 2 with its own message. A failed probe command gives rc 2 with its own message (not the
   `SHELLCHECK_BIN` "run uv sync" text). The `case` that reads the verdict continues ONLY on
   `ok)`; `*)` calls `infra`, which also catches an empty verdict from a renamed or missing
   function.
6. **D6 — No bypass flag.** No environment variable skips the probe. A bypass would only restore
   the silent hang. The syntax-only workaround in §6 stays available.
7. **D7 — Probe tool.** A Python one-liner through `uv run --locked --project py python3 -c`,
   the interpreter the gate already uses for `SHELLCHECK_BIN`. `PROTO_REPORTER=text` is already
   exported (`run.sh:31`). The capture ends in `| tail -n1`, the `SHELLCHECK_BIN` convention;
   `tail` reads all its input, so check 13 does not apply. The design does not merge the probe
   into the `SHELLCHECK_BIN` call, which is a documented fail-closed line with its own contract.
   This adds a second `uv run` to the full-gate path (about 0.3 s).
8. **D8 — On `ok`, print the value.** One stderr line: `actionlint gate: pipe capacity <N> bytes
   (floor 8192)`. CI then records the Linux value, and a green CI run shows that the probe ran.
9. **D9 — Removal is tied to the pin.** The follow-up (a new Linear issue) bumps actionlint to
   the first release that contains `fd33e9f582`. The probe must NOT simply be removed then,
   because mechanism 2 (bash) stays. The follow-up re-decides: keep the probe for mechanism 2
   with a reworded message, or remove it and record the bash residual. To stop the probe going
   stale without notice, the preflight asserts that `.prototools` still pins `actionlint =
   "1.7.12"`. A pin change then reds the gate with rc 2 and a message that points to this spec
   and the follow-up issue.

## 4. Components

### 4.1 `pipe_capacity_verdict` (pure function, `ci/actionlint/run.sh`)

- Input: one string, the probe output. Output on stdout: `ok`, `small` or `invalid`.
- `invalid`: empty; any character that is not a digit (the file's own
  `case "$v" in ''|*[!0-9]*)` idiom, which is bash 3.2 safe and not line-oriented); more than 9
  digits (bash arithmetic wraps); or a value of zero after `$((10#$v))` (so `0` and `00` are
  both invalid, and `0512` is not read as octal).
- `small`: below `PIPE_CAPACITY_FLOOR` (8192). `ok`: at or above it.
- No subprocess, no file access.

### 4.2 `pipe_capacity_preflight` (function, called once at column 0)

- Called by one column-0 line directly after the argument `case`:
  `[ "$SELF_TEST_ONLY" = 1 ] || pipe_capacity_preflight`. A function keeps its call line at
  column 0 for the pin (§4.5). `infra` inside a plain function exits the whole script; only a
  `done < <(...)` subshell is a problem (`run.sh:2075-2086`), and the preflight uses none.
- Step 1, the pin assertion (D9): read `.prototools`; if the line `actionlint = "1.7.12"` is not
  present, `infra` with the stale-probe message.
- Step 2, the probe: create a pipe, set `O_NONBLOCK` on the write end, write one byte at a time
  until `BlockingIOError` or 1 MiB, print the count. One byte at a time measures the real
  capacity and does not depend on the partial-write rule (M5). The 1 MiB limit stops the probe
  itself from running forever.
- Step 3: capture the status of the probe separately from its output, then route: status not 0
  → `infra` (probe-failed message); then `case` on the verdict: `ok)` prints D8's line; `small)`
  → `infra` (small message); `*)` → `infra` (invalid message).
- Keep the probe as close to check 1 as D1 permits: it is the first action of the full gate
  after argument parsing. Nothing sits between the probe and `run_self_tests`.

### 4.3 Messages

- `small`: `actionlint gate: INFRASTRUCTURE ERROR: a new pipe on this host holds only <N> bytes
  (floor 8192). With pipes this small, the gate cannot finish: bash 5.x here-strings in its own
  self-tests deadlock above <N> bytes, and the pinned actionlint writes each run: script into
  shellcheck's stdin before it starts shellcheck (rhysd/actionlint#650). No check ran. See
  ci/actionlint/README.md, "Small pipes on macOS".` The message names no actionlint version and
  no release state, so a pin change does not make it wrong.
- `invalid`: `... INFRASTRUCTURE ERROR: the pipe-capacity probe printed '<output>', not a
  positive integer. No check ran.`
- probe failed: `... INFRASTRUCTURE ERROR: the pipe-capacity probe failed (rc <rc>) via 'uv run
  --locked --project py'. No check ran.`
- stale probe: `... INFRASTRUCTURE ERROR: .prototools no longer pins actionlint 1.7.12. The
  pipe-capacity probe (SMA-612) exists only for that version. Re-decide it per
  docs/superpowers/specs/2026-09-19-sma-612-actionlint-pipe-capacity-design.md D9.`

### 4.4 `pipe_capacity_self_test` (the sixteenth fixture table)

Same shape as `kill_predicate_self_test` (`run.sh:4538-4569`). Its call line in `run_self_tests`
must be exactly two spaces plus `pipe_capacity_self_test`, with nothing after it, because check
9's awk (`run.sh:5313`) accepts only that form.

| Input | Expected |
|---|---|
| `512` | `small` |
| `8191` | `small` |
| `8192` | `ok` |
| `65536` | `ok` |
| `08192` | `ok` (not octal) |
| `0` | `invalid` |
| `00` | `invalid` |
| `` (empty) | `invalid` |
| `abc` | `invalid` |
| `-1` | `invalid` |
| `512 ` (trailing space) | `invalid` |
| `abc` + newline + `65536` | `invalid` (multi-line) |
| `{"type":"message"}` | `invalid` (an NDJSON preamble past `tail -n1`) |
| `1234567890` (10 digits) | `invalid` (overflow guard) |
| `999999999` (9 digits) | `ok` |

Registration: `SELF_TEST_COUNT` 15 → 16 (`run.sh:48` and its name-list comment), one call line
in `run_self_tests`. Check 7 and check 9 then cover the table with no further change.

### 4.5 Call-site pins (`ci/affected-graph/ci_targets.py`)

`ACTIONLINT_SH_CALL_SITES` is a whole-line, column-0 pin (`ci_targets.py:1704-1714`, reason at
`:770-789`), not a substring pin. A commented-out copy does not satisfy it. Its real residual is
the actionlint README's L10 (`ci/actionlint/README.md:248-254`): a live-looking copy in an
unindented `if false; then … fi` block or heredoc.

Pins, following the check-10 precedent (`ci_targets.py:857-880`, which pins the call, the status
capture and both routing arms):

- Column 0 (`ACTIONLINT_SH_CALL_SITES`): the call line `[ "$SELF_TEST_ONLY" = 1 ] ||
  pipe_capacity_preflight`.
- Indented (`ACTIONLINT_SH_INDENTED_CALL_SITES`, stripped on both sides): inside the preflight,
  the probe capture line, the status-routing line, the `case` line on the verdict, the `small)`
  arm and the `*)` arm, and the `.prototools` pin assertion.

Each new entry needs, in the same file:
1. the same line in the hand-written `wired_actionlint` fixture (`ci_targets.py:2456-2535`), or
   the "fired on a wired tree" assertion (`:2593-2597`) reds `repo:affected-smoke`;
2. a deletion row and an indentation row, by the convention at `ci_targets.py:2673-2850`.

## 5. Error handling summary

| Situation | Result |
|---|---|
| `.prototools` pin is not `actionlint = "1.7.12"` | rc 2, stale-probe message. |
| Probe command fails (uv missing, Python error) | rc 2, probe-failed message. |
| Probe output not a valid positive integer | rc 2, invalid message. |
| Capacity < 8192 | rc 2, small message. No self-test and no check runs. |
| Capacity ≥ 8192 | One stderr line with the value, then the gate runs as today. |
| `--self-test` mode | No probe. The fixture table runs. |

## 6. Documentation

- `ci/actionlint/README.md`: a new section "Small pipes on macOS": both mechanisms, M1–M10 in
  short form, the upstream link, and the syntax-only workaround (`actionlint -shellcheck=
  -pyflakes= <file>`, stated plainly as NOT the gate: it skips shellcheck and every other check).
  A new Limitations entry for R1–R4. A paragraph "SMA-612 added a SIXTEENTH self-test". Update
  the "State: CURRENT" text (`:774-783`) and the cost paragraph that says one `uv run` on the
  full-gate path (`:785-793`); there are now two.
- Every count of fifteen tables / sixteen subprocesses: `README.md` 38, 45, 755-757, 774-777,
  802, 805; `run.sh` `usage()` 66-72, 5227 ("All FIFTEEN"), 5819 ("15 `uv run` invocations");
  `moon.yml` 689-696 and 709-713. In the new placement comment, name the early exit, not a line
  number (`run.sh:5816` already cites a stale ":4765").
- `CLAUDE.md`: a new gotcha entry for the 512-byte pipe, both mechanisms and the probe. Correct
  the "LOCAL ONLY, CORRECTED (SMA-512)" entry: under either bash, the full gate on a small-pipe
  host now exits rc 2 in seconds; `--self-test` under Homebrew bash still hangs (R2). Update the
  `SELF_TEST_COUNT` sentence (15 → 16, SMA-612 the sixteenth; "sixteenth-and-later" becomes
  "seventeenth-and-later").
- Memory: `macos-bash-535-herestring-deadlock.md`, `affected-smoke-hang-fixed-by-system-bash.md`
  (lines 66-67) and `gate-failures-from-wrong-bash.md`, plus their `MEMORY.md` index lines: add
  the measured 512-byte boundary (M9) and the common trigger.

## 7. Testing

All local runs name the bash they use.

1. Red-first: add the fixture table before `pipe_capacity_verdict` exists; `/bin/bash
   ci/actionlint/run.sh --self-test` fails on the new rows.
2. `/bin/bash ci/actionlint/run.sh --self-test`: the only failures are the two known false
   `cargo-lock-step` rows (CLAUDE.md, SMA-647). The new table passes.
3. Delete-the-feature (memory "red-first is not proof"): (a) make `pipe_capacity_verdict`
   always print `ok` → the table fails; (b) delete the call line in `run_self_tests` → check 7
   fails.
4. Full gate on this host, under BOTH `/bin/bash` and `/opt/homebrew/bin/bash`: rc 2 with the
   `small` message, no self-test output before it, no actionlint or shellcheck process left.
   Record the measured time; expect a few seconds (one `uv run` and one Python start).
5. Probe-failure paths, each under `/bin/bash` with a temporary edit that is reverted by Edit
   (not `git checkout`, memory "mutation restore discards the fix"): the capture prints `abc` →
   rc 2, invalid message; `uv` removed from PATH → rc 2, probe-failed message; `.prototools`
   pin changed to `1.7.13` → rc 2, stale-probe message.
6. Pin proof: delete the production call line → `ci/affected-graph/ci_targets.py`'s check reds
   (run it directly with `uv run --locked --project py python3 ci/affected-graph/ci_targets.py`
   or through `repo:affected-smoke` under `/bin/bash`). Restore it; green.
7. `ci_targets.py` self-test (`--self-test`) green with the new `wired_actionlint`, deletion and
   indentation rows. `repo:ruff-ci` over the changed Python.
8. CI: the full gate is green on Linux, and its log shows `pipe capacity <N> bytes` (D8), which
   proves the probe ran there and did not block.

## 8. Out of scope

- Building actionlint from source, or a container run (options 2 and 3, not chosen).
- Removing the here-strings from `run.sh` or any other gate. That would fix mechanism 2 for real,
  but it touches many gates and needs its own issue.
- Removing the 512-byte limit on the host.
- A watchdog timer around check 1 or the self-tests. It needs a time limit that is not a false
  red on a slow host, and that needs its own measurement.

## 9. Residual risks

- **R1.** The probe measures one pipe at one moment, and the state is probably global to the
  host (§2). A state change between the probe and check 1 can still give a hang; actionlint's
  own concurrent shellcheck pipes add to the pipe memory. No data gives this a probability. The
  probe is the first action of the full gate to keep the window short, but check 1 still comes
  after the self-tests and checks 8–8f.
- **R2.** `--self-test` under Homebrew bash on a small-pipe host still hangs silently (D2).
- **R3.** Pins: a live-looking copy of a pinned line in a never-executed block satisfies both pin
  tables (README L10). The capture line's inner Python text is pinned only as one line; a change
  inside the Python that still prints a large number is not caught by any pin.
- **R4.** The probe covers only the one measured trigger. Another cause of the same hang with a
  normal pipe size is not detected (no watchdog).
- **R5.** If the small-pipe state is a host state and not a Darwin default, a healthy host runs
  the gate as before, plus about 0.3 s and one stderr line.

## 10. Open questions for Sven

- **Q1.** Does a plain Terminal (not a Claude Code session) show the same 512-byte limit? One
  command answers it (512 = the small state; 16384 or more = healthy):

  ```sh
  /usr/bin/python3 -c $'import os,fcntl\nr,w=os.pipe();fcntl.fcntl(w,fcntl.F_SETFL,os.O_NONBLOCK);n=0\ntry:\n  while n<1<<20: n+=os.write(w,b"x")\nexcept BlockingIOError: pass\nprint(n)'
  ```

  The §2 table records the answer. The design does not depend on it.
- **Q2.** Which bash ran the issue's 560 s reproduction is not recorded. Under this design both
  bashes now give rc 2, so the answer no longer changes the fix.
