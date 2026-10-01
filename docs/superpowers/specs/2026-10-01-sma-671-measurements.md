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
