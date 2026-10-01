# SMA-671 measurements

Run: [36916864328](https://github.com/SMK1085/paigasus-core/actions/runs/36916864328), `workflow_dispatch` on
`feature/sma-671-images-console-coverage` at `0d03b19f23aff1016ae964585a8eaa90f04e8edd`, 2026-10-01.
`CONSOLE_PARITY_REQUIRED` unset. Both jobs passed: amd64 job `110552873839`, arm64 job
`110552874100`.

| ID | What | amd64 | arm64 |
|---|---|---|---|
| M1 | Parity, "Console release sequence", iam-console | staged tree matches the host build (22 files, public=0) | staged tree matches the host build (22 files, public=0) |
| M1 | Parity, "Console release sequence", gateway-console | staged tree matches the host build (22 files, public=0) | staged tree matches the host build (22 files, public=0) |
| M1 | Parity, "Build + smoke both consoles", iam-console | staged tree matches the host build (22 files, public=0) | staged tree matches the host build (22 files, public=0) |
| M1 | Parity, "Build + smoke both consoles", gateway-console | staged tree matches the host build (22 files, public=0) | staged tree matches the host build (22 files, public=0) |
| M2 | `df -h /` before / after the host build (Size, Used, Avail) | 145G, 41G, 104G / 145G, 43G, 102G | 145G, 35G, 110G / 145G, 37G, 109G |
| M2 | `pnpm install` time | 10 s (974 packages; pnpm reports "Done in 9.5s") | 10 s (974 packages; pnpm reports "Done in 10.1s") |
| M2 | `moon run` time | 48 s (moon reports 36.9 s; each build about 32 s) | 45 s (moon reports 33.2 s; each build about 28 s) |
| M2 | Host-build step / job duration | 58 s / 13 min 15 s | 55 s / 11 min 6 s |
| M3 | `git status --ignored=matching` after the build | `!!` only: `ts/node_modules/`, `node_modules/` of 2 apps and 10 packages, `.next/` of the 2 consoles. No `??` line. | the same 15 `!!` lines as amd64. No `??` line. |
| M4 | `moon run --upstream none` result | passed, rc 0, "Tasks: 2 completed" | passed, rc 0, "Tasks: 2 completed" |
| M5 | Toolchains that moon set up | one line: `installing proto 0.60.2` (about 11 s). No rust, python or uv line. | one line: `installing proto 0.60.2`. No rust, python or uv line. |
| M6 | Tracked files changed | none (no ` M` or `M ` line) | none (no ` M` or `M ` line) |

Other facts from the same logs:

- moon printed two `WARN` lines: "Detected a shallow checkout" and "falling back to an empty files
  list". The two named targets ran. The warning did not change the result.
- Next printed "No build cache found" for each console. Each build was a cold build.
- `.prototools` pins proto 0.61.1. moon installed proto 0.60.2 for its own use in the step. The
  step passed with it.
- "Build + smoke both consoles" took 13 s on amd64 and 10 s on arm64, after "Console release
  sequence" (2 min 8 s on amd64, 1 min 52 s on arm64).

Stop rules (plan Task 1 Step 8): no rule fired. M1 matches on all eight cells. M4 passed. M6 shows
no changed tracked file. The smallest Avail after the host build is 102G, more than 10 GB.

Decisions from these figures:

- Timeout of the host-build step: `max(20, ceil(1.5 × 1))` = 20 minutes. The slowest measured
  step took 58 s, which rounds up to 1 minute. Task 6 keeps `timeout-minutes: 20`.
- `.dockerignore` pattern: none is necessary. Each M3 path is under `node_modules` or `.next`.
  `ts/.dockerignore` already excludes `node_modules`, `**/node_modules` and `**/.next`.

## Item 3: the job figures of run 36347690901 (SMA-671 Q1)

Run [36347690901](https://github.com/SMK1085/paigasus-core/actions/runs/36347690901): `main` push,
`83d446fc`, 2026-09-27 (20:20:52 UTC). Both jobs passed: amd64 job `108700148019`, arm64 job
`108700148041`. This run predates SMA-675 (the Redis sidecar and the kernel rows of the console
smoke), so the current job does more work than this run.

| Leg | Disk size | Used at start / after reclaim / before release seq. / after it / after Build + smoke | "Console release sequence" | "Build + smoke both consoles" | Job |
|---|---|---|---|---|---|
| amd64 | 145 GB | 59 / 35 / 41 / 46 / 46 GB | 2 min 1 s | 6 s | 12 min 42 s |
| arm64 | 145 GB | 37 / 29 / 35 / 40 / 40 GB | 1 min 56 s | 6 s | 11 min 7 s |

The figures confirm spec F7 and F8. The amd64 disk size (the F8 gap) is 145 GB, the same as
arm64. The `df -h /` header and data lines of the amd64 log show it at each of the five points. So
the amd64 disk is not smaller than the arm64 disk. The amd64 leg starts with more used space (59
GB against 37 GB) and has less space free (99 GB against 105 GB after "Build + smoke both
consoles").

Q-c: yes, the 6 s of "Build + smoke both consoles" is a BuildKit cache hit, on both legs. The
`RUN cd "apps/<app>" && pnpm exec next build` step shows `CACHED` for iam-console and
gateway-console in that step, on amd64 and on arm64. The step has 28 `CACHED` lines on each leg.
Its 24 `DONE` lines take 0.0 s to 0.4 s each, so no `RUN` step did build work. The real Next
builds ran in "Console release sequence" just before: from 21.4 s to 24.4 s for each console. So the
job pays for ONE pair of Next builds, not two. The step reached `== CONSOLE SMOKE OK ==` on both
legs. In this run, the staged-tree parity row printed "staged-tree parity NOT CHECKED this run"
for both consoles on both legs, because the job had no host build.

## Item 4: the base-image digest watch item

F10, re-checked on 2026-10-01 against the Dependabot PR list of the repository:

- `ubuntu:24.04` (`rs/Dockerfile`): three digest refreshes, all merged. PR 163 (2026-08-27), PR
  245 (2026-09-17) and PR 288 (2026-09-22). The `origin/main` pin is `008173c`, the PR 288 digest.
- `rust:1.95.0-bookworm` (`rs/Dockerfile`): no Dependabot PR, open or closed.
- `node:24.16.0-bookworm` (`ts/Dockerfile`): no Dependabot PR, open or closed. The only `node`
  match is PR 28, an `@types/node` npm bump, which is not a base image.
- `gcr.io/distroless/nodejs24-debian12` (`ts/Dockerfile`, not in F10): no Dependabot PR.
- The two Dependabot `docker-minor-patch` group PRs (PR 299 closed, PR 305 merged on 2026-09-26) change
  only `ci/kind/manifests/`. They do not change a Dockerfile.

No `rust` or `node` digest-refresh PR exists. Item 4 stays open.

Linear comment: posted on SMA-671 on 2026-10-01 (comment `d1d2963f-29d5-4488-b83a-e3053cbb1c3a`).

## Mutation proof (SMA-671 AC 4)

Measured on 2026-10-01 in the SMA-671 worktree, under system `/bin/bash` 3.2.57, with
`/bin/bash ci/images/console-selftest.sh`. The baseline before the battery and the run after the
last restore both gave rc 0 with `289 passed, 0 failed, 0 skipped`. Each mutation was one Edit-tool
edit. `bash -n ci/images/run.sh` passed after each edit. Each restore was the reverse Edit-tool
edit. `git diff --stat` was empty after each restore. No mutation was committed.

| ID | Mutation | Row(s) that went red | Restored, `git diff` empty |
|---|---|---|---|
| MU1 | `console_staged_parity_row`, no-host-build arm: `if [ "$parity" = required ]; then` → `if false; then` | SP2 (rc 1, 288 passed) | yes |
| MU2 | `parity_required_flag`, `*)` arm body → `echo "--parity=optional"` | PF2, PF3, PF3-out, PF3b, PF3b-out, PF3c, PF3c-out, D2, D2-nodocker, D3, D3-nodocker (rc 1, 278 passed) | yes |
| MU3 | `smoke_consoles`, second-word `case`, `*)` arm body → `parity="optional"` | Z6, Z6b (rc 1, 287 passed) | yes |
| MU4 | `smoke_consoles`: delete `console_staged_parity_row "$app" "$image" "$parity" \|\| ec=1` | P1d, P1d-mut, Z1-parity-iam, Z1-parity-gw, Z2-parity-iam, Z2-parity-gw (rc 1, 283 passed) | yes |
| MU5 | `console_staged_parity_row`, third-argument `*)` arm body → `parity=optional` | SP2b (rc 1, 288 passed) | yes |
| MU6 | `console_staged_parity_row`, same-directories arm: `elif [ "$parity" = required ]; then` → `elif false; then` | SP4b (rc 1, 288 passed) | yes |
| MU7 | `docker_context_leaks`: `leaks=$((leaks + 1))` → `leaks=$((leaks + 0))` | CX1, CX2, CX3, CX4, CX6, CX7 (rc 1, 283 passed) | yes |
| MU8 | `all-consoles)` arm: delete `pr_flag="$(parity_required_flag)" \|\| exit 1` | D2, D2-nodocker (rc 1, 287 passed) | yes |
| MU9 | `smoke)` arm: delete `pr_flag="$(parity_required_flag)" \|\| exit 1` | D3 (rc 1, 288 passed). D3-nodocker stayed green | yes |
| MU10 | `images.yml`, "Console release sequence": delete `CONSOLE_PARITY_REQUIRED: '1'` | W1, W1-out, W1-m1-applied, W1-m3-applied, W1-m3, W1-m7-applied (rc 1, 283 passed) | yes |
| MU11 | `images.yml`, "Console release sequence": delete `set +e` | RS1, RS1-gw-build, RS1-gw-smoke, RS3, RS3-gw (rc 1, 284 passed) | yes |
| MU12 | `images.yml`: move the whole "Host build of both consoles" step after "Build + smoke both consoles" | W1, W1-out, W1-m4 (rc 1, 286 passed) | yes |

Every mutation reddened its expected row, so Step 3 (fix and re-run the whole battery) did not
apply. Notes on the rows:

- MU9: only D3 went red, with "rc 0, expected 1". D3-nodocker stayed green. With the line
  deleted, the `smoke)` arm stops at `run.sh` line 2625 with `pr_flag: unbound variable` (from
  `set -u`). That is before `smoke_consoles` and before any docker call, so the plan's "D3 (or
  D3-nodocker)" holds through D3. The rc 0 is a bash 3.2 behaviour, measured with a three-line
  script: under `set -euo pipefail` with an EXIT trap whose last command returns 0, an
  unbound-variable stop exits 0 under `/bin/bash` 3.2.57 and 1 under Homebrew bash 5.3.15. The
  top-level `trap load_oci_cleanup EXIT` (`run.sh` line 810) is such a trap. D3 also checks
  stderr for the `CONSOLE_PARITY_REQUIRED must be '0' or '1'` text, which this mutation does not
  print, so D3 goes red under bash 5 (CI) too. Not changed here: the bash 3.2 rc 0 on an
  unbound-variable stop is a local-only fail-open of `run.sh`, outside the SMA-671 scope.
- MU10: the in-harness W1 mutations m1, m3 and m7 also report "the mutation did not apply". They
  edit the line that MU10 deleted, so this is expected.
- The in-harness W1 mutations W1-m1 to W1-m9 passed on the baseline and on the run after the last
  restore. Those PASS rows are the proof for those targets.
