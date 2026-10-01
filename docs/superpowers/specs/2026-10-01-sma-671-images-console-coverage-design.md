# SMA-671: Widen the images workflow coverage for the console images

- **Linear:** [SMA-671](https://linear.app/smaschek/issue/SMA-671)
- **Branch:** `feature/sma-671-images-console-coverage`
- **Related:** SMA-513 (PR 279, `8528676e`), SMA-670 (PR 310, console self-test), SMA-688 (PR 311,
  console release sequence), SMA-675 (PR 339, `52d51f82`, the console smoke kernel probe)
- **Status:** APPROVED by Sven on 2026-10-01, with the decisions in §0. The Stage 1 agent wrote
  it unattended on 2026-09-27, and the spec-challenger reviewed it (APPROVE WITH CHANGES). On
  2026-10-01 the setup agent checked it again against `origin/main` `ecc87bbd` and corrected it.
  §12 lists each correction.

## 0. Approval decisions (Sven, 2026-10-01)

These decisions override the defaults of §10.

- **Q1:** yes. Record item 3 from the `main` push run 36347690901. Do not start a new manual
  dispatch for the old figures.
- **Q2:** the plan decides on a pnpm store cache. If the plan uses one, the cache key carries a
  workflow prefix and `runner.arch`.
- **Q3:** the host build runs on both legs, amd64 and arm64.
- **Q4:** `@paigasus/next-config` stays off the `images.yml` filter.
- **Q5:** `ts/apps/*/moon.yml` stays off the `images.yml` filter. The docs state the limit.
- **Q-a:** accepted. Each `images.yml` run, rs-only PRs included, pays for a TS install and a host
  Next build of both consoles on each leg that it runs.

No decision widens the scope of the issue.

## 1. Problem

The issue has four items. All concern `.github/workflows/images.yml` and its inputs.

1. `smoke_consoles` in `ci/images/run.sh` (the parity block is lines 2157-2247 on `origin/main`
   `ecc87bbd`) compares the staged `.next/static` of the image with the tree of a host build. CI
   makes no host build. So in CI the check always prints `staged-tree parity NOT CHECKED` and
   gates nothing (ruling R9 in PR 279). `docs/ops/RUNBOOK-containers.md` section 6 (lines
   486-491) says the same.
2. `@paigasus/next-config` is not on the `pull_request` filter of `images.yml`. Its
   `canonicalBasePath` can change the compiled `basePath`. `ts/Dockerfile` line 55 fails the build
   when the compiled value differs from the `BASE_PATH` build argument, and `base_path_for` in
   `ci/images/run.sh` (line 581) hardcodes that argument.
3. Nobody recorded the arm64 figures of the job with the two Next builds.
4. Watch item: Dependabot digest refreshes of `node:24.16.0-bookworm` (`ts/Dockerfile`) and of
   the `rust` tag (`rs/Dockerfile`).

## 2. Intake findings (2026-09-27, `origin/main` = `83d446fc`; re-checked 2026-10-01 at `ecc87bbd`)

Since `83d446fc`, one commit changed the files of this spec: `52d51f82` (SMA-675, PR 339). It
changed `ci/images/run.sh`, `ci/images/console-selftest.sh`, `images.yml`, `release.yml` and the
RUNBOOK. The line numbers below are from `ecc87bbd`. F16-F21 are new on 2026-10-01.

| ID | What | Result |
|---|---|---|
| F1 | `git log origin/main --grep SMA-671` | No commit. The issue is not done. |
| F2 | Parity arm in `smoke_consoles` | Unchanged in content since PR 279. SMA-675 moved it to lines 2157-2247. The "not checked" arm sets no error. |
| F3 | Callers of `smoke_consoles` in CI | `images.yml` lines 203 (`smoke "$key"` in "Console release sequence") and 222 (`all-consoles`); `release.yml` line 1252 (`smoke "${SERVICE}"` in "Smoke this service" of each console `images-build-<key>` job, with step `env` `PAIGASUS_SMOKE_KERNEL_CONTROL: 'off'`). |
| F4 | `chart.yml` `pull_request` filter | Lists `ts/packages/paigasus-next-config/**` (line 39) and the console apps. Its `kind` job runs `ci/kind/run.sh images`, which runs `ci/images/run.sh build-console` (kind run.sh line 366, `|| die_infra`). `build_console_one` passes `BASE_PATH` from `base_path_for` (run.sh lines 625-637). So a PR that changes `@paigasus/next-config` already runs `ts/Dockerfile` with the `basePath` check, on amd64. `ci/kind/run.sh` reports a `build-console` failure as rc 2, an infrastructure error (RUNBOOK lines 506-508). chart.yml runs no host build and no console smoke. |
| F5 | Moon inputs of the console tasks | `ts/apps/{iam,gateway}-console/moon.yml` list `/ts/packages/paigasus-next-config/src/**/*`, `package.json` and `tsconfig.app.json` in `build`, `typecheck`, `test` and `test-e2e`. The console `build` task has `deps: ['contracts:generate']` (gateway-console moon.yml line 114, iam-console line 125). The `build` script also compares the bundled `*paigasus_wasm_bg*.wasm` chunk with the COMMITTED `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm` (gateway-console moon.yml lines 84-97), so it needs no upstream wasm build. |
| F6 | Tests that pin the compiled value | `ts/packages/paigasus-next-config/tests/base-path.test.ts` line 7 pins `canonicalBasePath('/iam', 'test') === '/iam'`. `next-config.test.ts` line 27 pins `createNextConfig(...).basePath === '/iam'`. The console e2e specs use literal `/iam/...` paths (`ts/apps/iam-console/tests/e2e/login.spec.ts` line 17). `ts/apps/gateway-console/tests/standalone-runtime.test.ts` boots the BUILT gateway server and requests `/gateway/healthz` (lines 121-132). `gateway-console-ts:test` depends on `~:build` and lists the next-config sources as inputs. So the ordinary build pins both `/iam` and `/gateway`. `canonicalBasePath` is generic: it has no per-zone branch. |
| F7 | arm64 leg, run [36347690901](https://github.com/SMK1085/paigasus-core/actions/runs/36347690901) (push to `main`, `83d446fc`) | Disk: 37 GB used at job start, 29 GB after the reclaim, 35 GB before "Console release sequence", 40 GB after it, 40 GB after "Build + smoke both consoles". Size 145 GB. "Console release sequence" took 1 min 56 s. "Build + smoke both consoles" took 6 s. The job took 11 min 7 s (20:20:57 to 20:32:04 UTC). |
| F8 | amd64 leg, same run | 59 GB at job start, 35 GB after the reclaim, 41 GB before and 46 GB after "Console release sequence" (2 min 1 s). Job 12 min 42 s (20:20:55 to 20:33:37 UTC). The amd64 disk size is not yet recorded; the plan reads it from the same log (amd64 is the leg with the smaller disk). |
| F9 | Conclusions of the last 40 `images.yml` runs on `main` | 40 of 40 `success`. Each `main` push runs both legs. So the arm64 leg with the two Next builds has run and passed many times since PR 311. |
| F10 | Dependabot docker PRs | `ubuntu:24.04` digest refreshes in `/rs` merged as PR 163, 245 and 288 (the last on 2026-09-22). No digest-refresh PR for `rust:1.95.0-bookworm` or `node:24.16.0-bookworm` exists yet. |
| F11 | `moon run --help` (moon 2.5.3) | `--upstream <none|direct|deep>` exists. |
| F12 | Generated TS proto code | `ts/packages/paigasus-proto/src/generated/**` is committed. The Docker build uses the committed files. |
| F13 | `ts/.dockerignore` | Excludes `node_modules`, `**/node_modules`, `**/.next`, `**/.turbo`, `**/test-results`, `**/playwright-report`, `**/.env` and `**/.env.*`. It does NOT exclude other gitignored host artifacts. `git status --ignored` on a local checkout shows `ts/apps/*/tsconfig.tsbuildinfo` and `**/next-env.d.ts` outside those exclusions. So a host build CAN add files to the Docker context of the later image builds in the same job. `rs/crates/bindings` (the `bindings` build context) is not a pnpm workspace member. |
| F14 | `ci/images/console-selftest.sh` harness | `check_row` (lines 84-106) matches `present` and `absent` in stderr only. `expect_in` and `expect_not_in` (lines 582-597) already check a substring in any file, `$T/<row>.out` included (rows KC0, KC1 and Z1 use them on stdout). `run_fn` (lines 636-667) runs the function under `set -uo pipefail` and sets `ROOT="$FX_ROOT"`; `stub_reset` (lines 598-618) resets `FX_ROOT`. The stub docker already has a walk mode (`STUB_WALK_OUT`, `STUB_WALK_RC`, line 437), and the E rows share it. The E rows use an argv file (`$T/argv-config`, line 728). The harness header (lines 14-15) says `ROOT` is the only global that a copied function reads. That is already stale since SMA-675: `smoke_consoles` and `kernel_control_flag` are in `FUNCS` (line 36), and `smoke_case` (lines 1013-1031) sets the globals of `smoke_consoles`, `CONSOLE_STAGED_TREE_JS=""` included. |
| F15 | `images.yml` step shapes | "Console release sequence" loops over two keys under `set -euo pipefail` (lines 195-209), so a failure on `iam-console` stops `gateway-console`. "Build + smoke both consoles" (lines 218-224) keeps the rc and re-raises it after `df -h /`. `images.yml` runs no `moon` today. `ts/apps/*/moon.yml` is NOT on the `images.yml` filter. |
| F16 | The SMA-675 switch rule | SMA-675 added `PAIGASUS_SMOKE_KERNEL_CONTROL`. `kernel_control_flag` (run.sh lines 1545-1559) reads the variable at DISPATCH level and prints `--kernel-control=on\|off`. `smoke_consoles` takes that as its FIRST argument (lines 1814-1825): "It is an argument, not a global (the SMA-670 rule)". The comment above `kernel_control_flag` says "no row function reads the variable". Self-test rows KC0-KC3 test the helper, Z3 and Z4 test the argument parse, and D1 runs the real dispatch with a bad value and requires no docker call. |
| F17 | `images.yml` matrix | A `pull_request` run has ONE leg, amd64 (line 99). A `main` push and a `workflow_dispatch` run both legs. |
| F18 | Self-test row names | `R0`-`R4` are already the Redis sidecar rows (lines 828-840), and `P1a`-`P1c`, `P2`-`P16` are pins. `P1d` is free. |
| F19 | The three filter lists | They do NOT agree on `ecc87bbd`. RUNBOOK section 1 (lines 68-80) and `images.yml` (lines 58-68) carry the four SMA-675 smoke-runtime entries (`session.ts`, the two `layout.tsx`, the two probe pages). `rs/CLAUDE.md` "Container images" (lines 235-250) does not list them. |
| F20 | `run.sh smoke` with a cargo key | The "Smoke each service on its own" step (images.yml lines 154-157) calls `ci/images/run.sh smoke gateway` and `smoke iam`. The dispatch (run.sh, `smoke` arm) sends a cargo key to `smoke`, not to `smoke_consoles`, and only the console branch reads `PAIGASUS_SMOKE_KERNEL_CONTROL`. |
| F21 | `CONSOLE_STAGED_TREE_JS` | A script global (run.sh lines 1601-1621), defined just BEFORE the "SMA-670 smoke rows" section (line 1623). Only line 2176 reads it. `smoke_case` sets it to `""` (console-selftest.sh line 1021). |

The ticket is not blocked. Item 3 needs no new run: F7 and F9 give the figures from a `main`
push (Q1, approved). Item 2 needs no filter change and no new test (§3.2, Q4 approved). Item 1 is
the only code change. F19 adds one doc correction to `rs/CLAUDE.md`, so that AC 6 can hold.

## 3. Decisions

### D1. Item 1: make the parity check gate in `images.yml`

Add a host build of both consoles to the `images` job, and make the "not checked" arm a failure
when the caller asks for parity.

- **Opt-in by an explicit variable, not by `CI`, read at dispatch level** (corrected on
  2026-10-01, F16). SMA-675 set the rule for a smoke switch: a dispatch helper reads the variable
  and turns it into an ARGUMENT of `smoke_consoles`; no row function reads the variable. This
  design follows that rule.
  - A new helper `parity_required_flag`, beside `kernel_control_flag`, reads
    `${CONSOLE_PARITY_REQUIRED-}` (no default; `run.sh` line 21 and the harness `run_fn` both run
    under `set -u`). It parses the value strictly:
    - unset, empty or `0`: it prints `--parity=optional`. The "no host build" arm keeps its
      current behaviour (a message, no error).
    - `1`: it prints `--parity=required`. The "no host build" arm prints an `::error::` and the
      row returns 1.
    - any other value: it prints
      `::error::CONSOLE_PARITY_REQUIRED must be '0' or '1' (unset or empty means 0), not '<value>'.`
      to stderr and returns 1. So `true` or `yes` cannot turn the gate off silently.
  - The polarity is the opposite of `PAIGASUS_SMOKE_KERNEL_CONTROL`, where unset means "on". This
    is deliberate: `release.yml` has no host build, so an unset value must not red a release.
  - The `smoke` console arm and the `all-consoles` arm call it as
    `pr_flag="$(parity_required_flag)" || exit 1`, next to `kc_flag`, BEFORE any docker call.
    The `smoke` arm with a cargo key does not read it (F20).
  - `smoke_consoles` takes the flag as its SECOND argument and parses it as it parses the first:
    any value other than `--parity=required` or `--parity=optional` is a usage error, before the
    trap and before any docker call. It passes `required` or `optional` to the row function.
  Reason for a variable: `release.yml` line 1252 also calls `smoke` for a console, with `CI=true`
  and no host build (F3). A `CI`-keyed rule would red every console release.
- **Only `images.yml` sets the variable.** It sets it on the "Console release sequence" step and
  on the "Build + smoke both consoles" step, as step-level `env`. It does not set it at job level.
  The "Console image self-test" step must not inherit it, because the Z rows run the real
  `smoke_consoles` (F14).
- **A pin keeps the production switch in place** (guard-the-guard). A new self-test row reads
  `images.yml` and fails unless (a) each CONSOLE smoke step has `CONSOLE_PARITY_REQUIRED: '1'` in
  its own step `env`, and (b) the host-build step comes before the first of those steps. A console
  smoke step is a step whose `run:` calls `ci/images/run.sh all-consoles`, or calls
  `ci/images/run.sh smoke` with an argument other than the cargo keys `iam` and `gateway`. The
  "Smoke each service on its own" step (`smoke gateway`, `smoke iam`) is not a console smoke step
  and must not need the line (F20). See §4.3 row W1. Without this pin, a deleted `env` line, a
  moved `env` line or a value of `true` returns the check to "NOT CHECKED" (or, for `true`, to a
  usage error that a later edit can "fix" by deleting the line), and the job stays green. That is
  the exact defect this ticket fixes.
- **Host build command.** One new step, before "Console release sequence":

  ```bash
  pnpm --dir ts install --frozen-lockfile
  moon run iam-console-ts:build gateway-console-ts:build --upstream none
  ```

  `--upstream none` skips `contracts:generate` (F5, F11). The generator needs `buf` and the BSR,
  and a failed generate deletes `error_details_pb.ts` (memory: BSR rate limit). The host build
  then compiles the committed generated tree and the committed wasm, as the Docker build does
  (F5, F12). So the two builds
  read the same source. `moon run` also runs its own dependency install and its sync actions. The
  plan reads the step log and confirms that moon's install uses a frozen lockfile in CI. If it
  does, the plan drops the explicit `pnpm install` line. If it does not, the explicit line stays
  first, so moon finds an installed tree.
- **Step timeout.** The host-build step gets its own `timeout-minutes` (start value 20, set from
  the AC 5 measurement), as `chart.yml` does for each step.
- **Tool install.** The "Install crane, syft and uv" step also installs `moon`, `node` and
  `pnpm` through proto. `chart.yml` lines 98-102 are the precedent for `node` and `pnpm`.
- **Both legs (Q3, approved).** The host-build step has no `if:` on the architecture. A
  `pull_request` run has only the amd64 leg (F17), so a PR gates parity on amd64 only. A `main`
  push and a `workflow_dispatch` run gate it on amd64 and arm64.
- **Same architecture, same platform.** Each leg builds on its native runner, so the host build
  and the builder stage both run on linux on the same architecture. This removes the "platform
  split" risk that RUNBOOK section 6 (lines 492-500) lists for a macOS host.
- **A parity miss does not stop the other console.** The "Console release sequence" loop keeps an
  rc per key and exits after the loop, as "Build + smoke both consoles" already does. So a miss on
  `iam-console` still runs the build, smoke and SBOM floor of `gateway-console`, and the log shows
  both results.
- **What this gates, and what it does not.** `images.yml` runs on PRs that touch its filter
  (`ts/Dockerfile`, `ci/images/**`, the lockfile and others) and on every `main` push. On those
  PRs, parity now gates. `ts/apps/*/moon.yml` is NOT on the filter (F15), and `chart.yml`, which
  does cover `ts/apps/*/**`, runs no host build and no smoke. So a moon.yml-side staging drift
  (for example a removed `public/` copy or a partial copy) shows only on the first `main` push
  after the merge. `tests/standalone-staging.test.ts` catches only the coarse case (the staging
  removed). The RUNBOOK and `rs/CLAUDE.md` must state this limit and must not say "parity gates
  every PR". Sven decided (Q5) that `ts/apps/*/moon.yml` stays off the filter, and the docs state
  the limit.
- **Cost (Q-a, approved).** The step has no condition. Each `images.yml` run, rs-only PRs
  included, pays for the TS install and the two host Next builds on each leg that it runs. A
  condition would add a second place that must agree with the filter.

**Rejected alternatives.**

- Split item 1 into its own ticket. The change is one workflow step, one environment variable,
  one extracted function and one pin. It stays in this ticket.
- Key the rule on `CI=true`. It reds `release.yml` (see above).
- Run the host build with its upstream tasks. It adds the BSR dependency to a job that has none
  today, for no gain in coverage.
- A shape-only fallback (compare only top-level directories). The code comment at run.sh lines
  2207-2221 rejects it on purpose. This design keeps that ruling.
- Let the row function read `CONSOLE_PARITY_REQUIRED` itself (the 2026-09-27 version of this
  spec). It breaks the SMA-675 rule that no row function reads a switch variable (F16).
- Add the host build to `release.yml` as well. The release builds a commit that `images.yml`
  already gated on `main`. It would add a TS toolchain to each release job. Out of scope.

### D2. Item 2: `@paigasus/next-config` stays off the `images.yml` filter

The RUNBOOK section 1 rule: a file belongs on the filter when a change to it can break the image
build but not the ordinary build. The rule also excludes a file that is already a Moon task
`input`.

- `@paigasus/next-config` is a Moon `input` of the console `build`, `typecheck`, `test` and
  `test-e2e` tasks (F5). So the exclusion clause applies.
- A change that moves a compiled value reds the ordinary build. The primary control is the unit
  tests and the built-server tests: `base-path.test.ts` and `next-config.test.ts` pin `/iam`, the
  iam e2e specs use literal `/iam/...` paths, and `gateway-console`'s `standalone-runtime.test.ts`
  boots the built server at `/gateway/healthz` (F6). `canonicalBasePath` is generic, so a change
  that moves `/gateway` but not `/iam` is not a realistic mutation.
- The second control is `chart.yml`, which runs `ts/Dockerfile` with the `basePath` check on each
  PR that touches `ts/packages/paigasus-next-config/**` (F4). It reports a `build-console`
  failure as an infrastructure error (rc 2), in a workflow with known flakes, so it is a weaker
  signal than the unit tests. Both workflows are not required checks, so a second image build in
  `images.yml` adds no gate.

No new test is necessary. Sven confirmed this decision (Q4). The change adds the same sentence
to RUNBOOK section 1 and to `rs/CLAUDE.md`: `@paigasus/next-config` is off the filter, because it
is a Moon input, the unit and `standalone-runtime` tests pin both base paths, and `chart.yml` is
the second control.

Corrected on 2026-10-01 (F19): the three lists (the `images.yml` filter, RUNBOOK section 1 and
`rs/CLAUDE.md` "Container images") do NOT agree on `ecc87bbd`. SMA-675 added four smoke-runtime
entries to the filter and to the RUNBOOK, but not to `rs/CLAUDE.md`. So this change also adds
those entries (or one sentence that names the RUNBOOK's second clause and the four files) to the
`rs/CLAUDE.md` list. After that the three lists agree (AC 6).

### D3. Item 3: record the arm64 figures from run 36347690901

The RUNBOOK holds no job figures today (no PR 279 figures, no disk or time figures). So the
figures go to ONE place: the PR description, copied to a Linear comment on SMA-671. Record F7 and
F8 with the run id and the date, and add the amd64 disk size (F8 gap). Record also whether the
6 s of "Build + smoke both consoles" on arm64 is a BuildKit cache hit on the layers that "Console
release sequence" built just before it (Q-c). If it is, the figures must say that the job pays for
one pair of Next builds, not two.

Do not start a new `workflow_dispatch` run for the old figures: the `main` push already ran both
legs with the two Next builds (F9). Sven approved this (Q1). Run 36347690901 is from `83d446fc`,
before SMA-675 added the Redis sidecar and the kernel rows to the console smoke. The figures must
say so, because the current job does more work than that run.

After this change lands, the first `main` run gives new figures with the host build. The
implementer records those too, in the same two places (AC 5).

### D4. Item 4: the watch item stays open

No code change. Record F10 in the PR description: the `ubuntu:24.04` versioned tag got three
digest refreshes, so the suppression experiment did not stop a digest-only refresh on that
versioned tag. No refresh for `rust` or `node` has appeared yet. The issue says to close the item
only when such a PR appears or the experiment state is known. So the implementer leaves a Linear
comment with F10 and does not close item 4. The "Done when" list of the issue does not include
item 4.

## 4. Design

### 4.1 `ci/images/run.sh`

- Extract the parity block (lines 2157-2247 on `ecc87bbd`) into a new function
  `console_staged_parity_row <app> <image> <required|optional>`, placed in the "SMA-670 smoke
  rows" section (line 1623). It follows that section's contract: it runs as `fn … || ec=1`, and it
  checks the rc of each command itself.
- **Scope of the extraction.** The block is NOT copied byte-for-byte, because it writes `ec=1` and
  uses locals that `smoke_consoles` declares (run.sh lines 1809-1811). Inside a function, `ec=1`
  would write the caller's `local ec` through bash dynamic scoping, and in the harness it would
  write a global. So:
  - The function declares `local rc=0` and every variable it uses as `local`: `img_out`,
    `img_public`, `img_list`, `host_public`, `host_list`, `img_dirs`, `host_dirs`, `host_std`,
    `host_static`, `host_id`, `img_rc`, and the new `parity` (the third argument).
  - Each `ec=1` in the block becomes `rc=1`. The function ends with `return "$rc"`.
  - The plan removes from the `local` lines of `smoke_consoles` each variable that only the
    parity block used. A variable that another part of `smoke_consoles` also uses stays there.
  - All message texts stay the same, except the two arms named below.
- **`CONSOLE_STAGED_TREE_JS` moves into the function** as a `local`, with its comment (run.sh
  lines 1601-1621, F21). Only run.sh line 2176 uses it. This matches the section contract "Each
  one keeps its JS program in a `local`" (line 1626) and needs no second awk rule in the harness.
  The plan also removes `CONSOLE_STAGED_TREE_JS=""` from `smoke_case` (console-selftest.sh line
  1021). After this change the function reads the global `ROOT` only. The harness header (lines
  14-15) is updated: it is already stale since SMA-675 (F14), so the new text names `ROOT` for the
  row functions and says that `smoke_case` sets the globals of `smoke_consoles`.
- **The dispatch helper `parity_required_flag`**, placed directly after `kernel_control_flag`,
  per D1. Its comment states the polarity and the reason (release.yml has no host build).
- **`smoke_consoles` takes `--parity=required|optional` as its second argument**, per D1. The
  `shift` after the first-argument parse becomes a parse of the second word, then `shift`. A
  missing or other value prints
  `::error::smoke_consoles: the second argument must be --parity=required or --parity=optional, not '<value>'.`
  and returns 1, before the trap. The `smoke` console arm (line 2453 onward) and the
  `all-consoles` arm pass `"$kc_flag" "$pr_flag" "$@"`.
- `smoke_consoles` calls the row as
  `console_staged_parity_row "$app" "$image" "$parity" || ec=1`.
- **The third argument is checked first**, at the top of the row: `required` or `optional`; any
  other value prints an `::error::` and returns 1 with no docker call. The value never comes from
  the environment.
- **The "no host build" arm:**
  - Required: print
    `::error::${app}: no host build at ${host_static}, and parity is required (CONSOLE_PARITY_REQUIRED=1) — staged-tree parity was required but NOT checked. Run 'moon run ${app}-ts:build --upstream none' before the smoke.`
    to stderr and return 1.
  - Not required: the current message (stdout), updated so it no longer says "CI never runs a host
    build". It says that `images.yml` runs the host build with `CONSOLE_PARITY_REQUIRED=1`, and
    that this run did not.
- **The "DIFFERENT sources" arm** (same directories, different files; line 2227). Its current
  text tells the reader to re-run `moon run <app>-ts:build`. In `images.yml` the host build is
  always fresh and from the same commit, so that advice is wrong there. When parity is required,
  the arm prints a different `::error::`: the host build and the image build are from the same
  commit, so this is most probably the chunk-name assumption failing (RUNBOOK section 6, second
  bullet), not a Dockerfile-versus-moon.yml drift. The message names the RUNBOOK section. When
  parity is not required, the current text stays.
- Update the comment above the "no host build" arm (lines 2236-2245). It must no longer say that
  CI never checks parity.
- Header usage block (run.sh lines 11-20): document `CONSOLE_PARITY_REQUIRED` (values `0`, empty
  or unset, and `1`), next to a line for `PAIGASUS_SMOKE_KERNEL_CONTROL`.

### 4.2 `.github/workflows/images.yml`

- "Install crane, syft and uv" also runs `proto install moon`, `proto install node` and
  `proto install pnpm`. Rename the step.
- New step "Host build of both consoles (staged-tree parity reference)" directly after "Console
  image self-test" (lines 180-181) and before "Console release sequence" (lines 195-209). It
  prints `df -h /` before and after. It has its own `timeout-minutes`. It has no `if:`, so it runs
  on each leg (Q3). Body as in D1.
- The new step then checks the Docker context. It runs
  `git status --porcelain --ignored --untracked-files=all -- ts rs/crates/bindings` and fails
  when a path appears that the context does not exclude. `ts/` is filtered by `ts/.dockerignore`.
  `rs/crates/bindings` is the SMA-634 named context `bindings` (run.sh `build_console_one`); a
  sub-directory context carries no `.dockerignore`, so ANY new file there enters it (added on
  2026-10-01). The plan decides the exact filter (for example, a
  `git check-ignore`-style match against the `.dockerignore` patterns, or an allow-list of the
  known artifacts). If a leak appears (for example `tsconfig.tsbuildinfo` or `next-env.d.ts`,
  F13), the plan picks one fix and states it: add the pattern to `ts/.dockerignore`, or delete the
  file after the host build. `release.yml` builds from a clean checkout, so the images of the two
  workflows must not get different contexts.
- `CONSOLE_PARITY_REQUIRED: '1'` as step `env` on "Console release sequence" and "Build + smoke
  both consoles". Not on "Smoke each service on its own" (cargo keys, F20), and not at job level.
  The value is quoted, as `release.yml` quotes `'off'`.
- "Console release sequence": keep an rc per key, run both keys, then `df -h /`, then exit with
  the combined rc (D1).
- Update the comments of those steps. `branches:` and `paths:` stay block sequences.
- `timeout-minutes: 60` for the job stays. The job took 11-13 minutes in run 36347690901 (F7,
  F8), before SMA-675 added the Redis sidecar and the kernel rows; the plan reads a current `main`
  run for the new base figure. `images.yml` is a separate workflow, so the 30-minute limit of the
  `ci.yml` `moon ci` job (which is near its limit on wide graphs) does not apply to it. The host
  build must not move into `ci.yml`.
- Optional: a pnpm store cache as in `chart.yml` lines 104-114. The plan decides (Q2, approved
  as "the plan decides"), from a measured install time. If the plan adds it, the key must include
  a workflow prefix and
  `runner.arch`, for example `images-pnpm-${{ runner.os }}-${{ runner.arch }}-<lockfile hash>`.
  `chart.yml`'s key (`pnpm-${{ runner.os }}-…`, line 112) is the same on amd64 and arm64 and
  would collide across workflows; on an exact-key hit `actions/cache` skips the save (memory).

### 4.3 `ci/images/console-selftest.sh`

Corrected on 2026-10-01: the row names, the stdout check and the switch rows follow the harness
as SMA-675 left it (F14, F16, F18).

- Add `console_staged_parity_row` and `parity_required_flag` to `FUNCS` (line 36).
- **No change to `check_row`.** The rows that must prove a stdout line ("NOT CHECKED" and
  "staged tree matches") use the existing `expect_in` and `expect_not_in` helpers on
  `$T/<row>.out`, as KC0, KC1 and Z1 do (F14). The 2026-09-27 version of this spec added an
  eighth argument to `check_row` and `run_fn`; that is not necessary.
- **A new argv file.** `$T/argv-parity` holds
  `run --rm --entrypoint /nodejs/bin/node $T_IMAGE -e <multi-line> /app/apps/iam-console --`,
  in the same form as `$T/argv-config` (line 728). Each row that reaches docker compares with it.
  This pins the SMA-688 rule that a row never builds an image name from `<app>`. Rows that must
  not reach docker use `none`.
- **The fixture root.** `run_fn` sets `ROOT="$FX_ROOT"`, and `stub_reset` resets `FX_ROOT`. So
  each parity row sets `FX_ROOT` (not `ROOT`) to its own temp fixture directory after
  `stub_reset`, and builds the fake host tree under it. The parity rows share `STUB_WALK_OUT` and
  `STUB_WALK_RC` with the E rows; each row sets them after `stub_reset`.
- **No row reads the environment for the parity mode.** The harness runs
  `unset CONSOLE_PARITY_REQUIRED` once at the top, so a developer's shell or a future job-level
  `env` cannot change a row. The helper rows set the variable inside their own wrapper, as
  `kc_set` and `kc_unset` do (lines 993-994).
- Parity rows. The prefix is `SP`, because `R0`-`R4` are already the Redis rows (F18).
  "stdout" means an `expect_in` on `$T/<row>.out`; "stderr" means the `present` argument of
  `run_fn`.
  - **SP1** host tree absent, mode `optional`: rc 0, stdout holds `NOT CHECKED`, stderr absent
    `::error::`, argv `none`.
  - **SP2** host tree absent, mode `required`: rc 1, stderr holds `parity was required but NOT
    checked`, argv `none`.
  - **SP2b** mode `maybe` (a bad third argument): rc 1, an `::error::` on stderr, argv `none`.
  - **SP3** host tree present and equal to the stub walk, mode `required`: rc 0, stdout holds
    `staged tree matches the host build`, stderr absent `::error::`, argv `$T/argv-parity`.
  - **SP3b** same as SP3 with mode `optional`: rc 0, same output.
  - **SP4** host tree with a different top-level directory: rc 1, stderr holds `different
    top-level directories`.
  - **SP4b** same directories, different files, mode `required`: rc 1, stderr holds the new
    required-mode text of the "DIFFERENT sources" arm (it names the chunk-name assumption), and
    absent `Re-run 'moon run`.
  - **SP4c** same as SP4b with mode `optional`: rc 1, stderr holds `DIFFERENT sources`.
  - **SP4d** `public=` mismatch (walk says `public=1`, host has no `public/`): rc 1, stderr holds
    `disagree on staging public/`.
  - **SP5** stub walk exits 1 (node ENOENT): rc 1, stderr holds `the staging copy in
    ts/Dockerfile did not run`.
  - **SP5b** stub walk exits 125: rc 1, stderr holds `NOT checked — docker exited 125`.
  - **SP5c** stub walk exits 0 with empty output: rc 1, stderr holds `printed nothing`.
  - **SP5d** host tree without `.next/BUILD_ID`: rc 1, stderr holds `has no .next/BUILD_ID`.
- Helper rows, in the shape of KC0-KC3 (lines 993-1005):
  - **PF0** variable unset: rc 0, stdout holds `--parity=optional`.
  - **PF0b** variable empty: rc 0, stdout holds `--parity=optional`.
  - **PF0c** variable `0`: rc 0, stdout holds `--parity=optional`.
  - **PF1** variable `1`: rc 0, stdout holds `--parity=required`.
  - **PF2** variable `true`: rc 1, stderr holds `CONSOLE_PARITY_REQUIRED must be '0' or '1'`.
- `smoke_consoles` rows (the Z rows, lines 1007-1063):
  - `smoke_case` stubs `console_staged_parity_row` like the other rows, and the stub records its
    third argument in `$Z_CALLS`. It no longer sets `CONSOLE_STAGED_TREE_JS`.
  - Z1, Z2 and Z5 pass `--parity=optional` (or `--parity=required`) as the second word. One of
    them checks with `expect_in` that the row got that mode for each zone.
  - **Z6** a missing or bad second word (`--parity=maybe`): rc 1, stderr holds `the second
    argument must be --parity=required or --parity=optional`, argv `none`.
- **D2** the dispatch arm through the REAL script, as row D1 does (lines 1074-1080):
  `CONSOLE_PARITY_REQUIRED=bogus` with `all-consoles` must exit 1 with the helper's message and
  make no stub docker call.
- **P1d** a new `pin_rows` entry that pins the call line
  `console_staged_parity_row "$app" "$image" "$parity" || ec=1` in the copied `smoke_consoles`.
- **W1** the workflow pin (D1). It reads `.github/workflows/images.yml` with awk (no YAML
  library, bash 3.2-safe) and checks: (a) every console smoke step (D1: a step whose `run:` calls
  `ci/images/run.sh all-consoles`, or `ci/images/run.sh smoke` with an argument other than `iam`
  and `gateway`) has the exact line `CONSOLE_PARITY_REQUIRED: '1'` in its own step `env:`; (b) at
  least one such step exists; (c) the host-build step (matched by its
  `moon run iam-console-ts:build` line) comes before the first such step; (d) the "Smoke each
  service on its own" step (`smoke gateway`, `smoke iam`) is NOT counted as a console smoke step.
  Each check has its own mutation in the harness, on a temp copy of the file: delete the `env`
  line, move it to another step, change `'1'` to `true`, move the host-build step after the
  release sequence, and change the cargo step to `smoke iam-console`. Each mutation must red W1
  with its own message. The self-test runs on every `images.yml` change, because `images.yml` and
  `ci/images/**` are on its own filter.
- Each row keeps the harness rules: bash 3.2 and bash 5 both run it, no `mapfile`, no
  `declare -A`, no here-string.

### 4.4 Documentation

- `docs/ops/RUNBOOK-containers.md`:
  - Section 1: the `@paigasus/next-config` sentence (D2), with the unit and `standalone-runtime`
    tests named as the primary control and `chart.yml` as the second one.
  - Section 6, lines 486-491 ("The check runs locally only"): rewrite. `images.yml` now runs a
    host build on each leg and sets `CONSOLE_PARITY_REQUIRED=1`, so the check gates there: on
    amd64 on the PRs its filter selects, and on amd64 and arm64 on `main` and on a dispatch
    (F17). It does not gate a PR that changes only `ts/apps/*/moon.yml` (D1, Q5). `release.yml`
    does not set the variable and says "NOT CHECKED" on purpose. Name the variable next to
    `PAIGASUS_SMOKE_KERNEL_CONTROL` (RUNBOOK lines 473-476) and state the opposite polarity.
  - Section 6, lines 492-500 (the chunk-name assumption): the "platform split" risk does not apply
    in CI, because both builds run on the same linux runner. Add a response: when the required
    "DIFFERENT sources" error appears in `images.yml`, read it as the assumption failing. The
    rollback is a reviewed PR that removes the two `CONSOLE_PARITY_REQUIRED` `env` lines AND
    updates row W1 in the same PR. Do not delete `.next` to silence it.
- `rs/CLAUDE.md` "Container images" (lines 226-260): one sentence for the `@paigasus/next-config`
  decision, one for the parity coverage limit (D1, last bullet), and the four SMA-675
  smoke-runtime filter entries that the list lacks (F19, D2).
- A doc that names the moon CI report file needs a `moon-diagnosis` marker (`repo:actionlint`
  check 12). None of the docs above should need to name it; if one does, add the marker.

## 5. Acceptance criteria

1. On a PR that triggers `images.yml`, the amd64 leg (the only PR leg, F17) prints `staged tree
   matches the host build` for both consoles in "Console release sequence" and in "Build + smoke
   both consoles". A `workflow_dispatch` run on the branch prints the same on amd64 and arm64.
   Neither prints a `NOT CHECKED` line.
2. With `--parity=required` and no host build, `console_staged_parity_row` fails with the new
   `::error::` (row SP2). A `CONSOLE_PARITY_REQUIRED` value other than `0`, `1`, empty or unset
   fails in `parity_required_flag` (row PF2) and, through the real dispatch, before any docker
   call (row D2).
3. Without the variable, the smoke behaves as today (rows SP1, SP3b, SP4c, PF0). `release.yml`
   needs no change and stays green.
4. `ci/images/console-selftest.sh` passes under `/bin/bash` 3.2 and under bash 5 in CI. A mutation
   that deletes the required branch reds SP2. A mutation that deletes the strict-value branch of
   the helper reds PF2. A mutation that deletes the call line reds P1d. Each of the five W1
   mutations reds W1.
5. A `workflow_dispatch` run on the branch passes on amd64 and arm64 (Q3). The PR description
   records, for each leg: `df -h /` before and after the host-build step, the step duration (moon cold
   start, install and the two Next builds included), and the job duration.
6. The `@paigasus/next-config` decision is in RUNBOOK section 1 and in `rs/CLAUDE.md`. The three
   lists of the filter agree.
7. The arm64 figures of F7 and the amd64 figures of F8 (with the disk size) are in the PR
   description and in a Linear comment, with the run id and the note that the run predates
   SMA-675 (Q1, D3).
8. `repo:actionlint` passes on the changed `images.yml`.
9. The host-build step's Docker-context check passes: no host artifact outside the
   `ts/.dockerignore` exclusions exists under `ts/`, and no new file exists under
   `rs/crates/bindings`, after the host build.
10. A parity failure on `iam-console` in "Console release sequence" still runs `gateway-console`
    (the step log shows both keys).

## 6. Test strategy

- **Measure R-a first.** Before any change to `run.sh` or the self-test, the plan starts one
  `workflow_dispatch` run of a branch that adds ONLY the host-build step (and the tool install),
  with the variable unset. A dispatch is necessary, because a PR run has the amd64 leg only
  (F17). The log then shows, on both legs, whether the existing parity arm
  prints "matches" or a mismatch. If it prints a mismatch, the implementer stops and reports
  (R-a). AC 1 depends on this result.
- **Unit:** the self-test rows SP1-SP5d, PF0-PF2, Z6, D2, P1d and W1 in `console-selftest.sh`,
  which CI runs in the "Console image self-test" step. The existing Z, KC and D1 rows stay
  green.
- **Mutation proof** (memory: "red-first is not proof"; authorized by Sven on 2026-09-28): after
  the implementation, delete the required branch, the strict-value branch of the helper, the
  second-argument parse, the call line and each W1 target one at a time, and
  confirm that the named row reds. Each mutation must compile (`bash -n`) so that it proves the
  row, not a syntax error. Re-run the whole battery after any fix.
- **Integration:** a `workflow_dispatch` run of `images.yml` on the branch, both legs (AC 1, 5).
  Read the log for the "matches the host build" line on each console and each leg.
- **Local:** `ci/images/console-selftest.sh` under `/bin/bash` 3.2. If the Mac is in the
  512-byte small-pipe state (SMA-612), run the bash-5 gates in a Linux container. On macOS the
  chunk names can differ from the linux builder (RUNBOOK risk 3). A local mismatch is therefore
  not proof of a CI defect. The CI run is the proof.
- **Full gate graph:** the `moon ci` target list from the root `CLAUDE.md` before the push,
  including `:actionlint` (bash 5 with a healthy pipe, or a Linux container).

## 7. Files expected to change

- `.github/workflows/images.yml`
- `ci/images/run.sh`
- `ci/images/console-selftest.sh`
- `docs/ops/RUNBOOK-containers.md`
- `rs/CLAUDE.md`
- Possibly `ts/.dockerignore` (if the Docker-context check finds a leak).
- Possibly `ci/actionlint/README.md` or its fixtures, if a check there pins the text or the step
  list of `images.yml` (the plan measures this with a grep).

## 8. Risks

- **R-a. Chunk names differ between the host build and the builder stage.** The builder runs
  `pnpm install --filter "@paigasus/${APP}..."` in `/app`. The host runs a full install in the
  checkout path. SMA-513 measured equal names from a macOS host with a different path, so a
  path-dependent hash is not expected. The plan measures this first (§6). If the first CI run
  shows a mismatch, the implementer stops and reports; the design does not add a fallback.
- **R-b. Build time.** A cold moon start (toolchain plugins, node and pnpm setup, moon's own
  install, `SyncWorkspace` and `SyncProject`, `contracts` included), a `pnpm install` and two
  Next builds on each leg. The step runs on every `images.yml` trigger, rs-only PRs included
  (Q-a, accepted). The job budget is 60 minutes. The job took about 12 minutes in run
  36347690901, before SMA-675 added the Redis sidecar and the kernel rows to the smoke. The step
  has its own timeout. AC 5 measures it.
- **R-c. Host artifacts enter the Docker context.** `.dockerignore` filters the context, not
  `.gitignore`. A host artifact that `.dockerignore` does not exclude (F13) enters every later
  image build in the job, and `release.yml` builds from a clean checkout. The host-build step
  checks this (§4.2, AC 9). `git status` without `--ignored` does not show these files, so it is
  not a sufficient check.
- **R-d. `--upstream none` skips a needed upstream.** The console build needs no built package:
  the packages are source-only and the wasm artifacts are committed (the `build` script compares
  the bundled wasm chunk with the committed file, F5). The napi binding is not built on the host;
  the plan confirms that `next build` does not need it. If `next build` fails for a
  missing upstream output, the plan adds only that task with `--upstream direct` or an explicit
  target, never `contracts:generate`.
- **R-e. A moon.yml-side drift shows only after the merge.** See D1, last bullet. Sven accepted
  this limit (Q5).

## 9. Out of scope

- Parity in `release.yml`.
- A new filter entry for any other `ts/packages/*` package. Each one is a Moon input of the
  console build, so the same rule excludes it.
- New `/gateway` base-path unit tests. The ordinary build already pins `/gateway` (F6).
- Closing item 4 of the issue.

## 10. Open questions (all answered on 2026-10-01)

- **Q1.** Is it acceptable to record item 3 from run 36347690901 (a `main` push) instead of the
  `gh workflow run images.yml --ref main` dispatch that the issue names? The figures come from
  the same workflow on `main` with the two Next builds. The design assumes yes.
  **ANSWERED (Sven, 2026-10-01): yes. Record item 3 from run 36347690901; no new manual
  dispatch.**
- **Q2.** Should the images job cache the pnpm store (as `chart.yml` does)? The design leaves it
  to the plan, decided on the measured install time. If yes, the key must carry a workflow prefix
  and `runner.arch` (§4.2).
  **ANSWERED (Sven, 2026-10-01): the plan decides. If the plan uses a cache, the key carries a
  workflow prefix and `runner.arch`.**
- **Q3.** Should the host build run on both legs, or on amd64 only? The design runs it on both,
  because the arm64 image is a released artifact too. It costs a second TS toolchain install on
  arm64.
  **ANSWERED (Sven, 2026-10-01): both legs, amd64 and arm64.**
- **Q4.** Item 2: Sven asked to "check the rule first". The design keeps the package off the
  filter because it is a Moon input, the ordinary build pins both base paths, and `chart.yml`
  already builds the image on such a PR (F4, F6). Does Sven want `images.yml` to cover it anyway,
  for example to get the arm64 build on the same PR? The design assumes no.
  **ANSWERED (Sven, 2026-10-01): no. `@paigasus/next-config` stays off the filter.** (A PR run
  has no arm64 leg anyway, F17.)
- **Q5.** Should `ts/apps/*/moon.yml` go on the `images.yml` filter, so that a moon.yml-side
  staging drift gates the PR and not only `main`? It adds an `images.yml` run (two legs of rs and
  console builds) to each console `moon.yml` edit, and the RUNBOOK section 1 rule would need an
  extension, because `moon.yml` is itself Moon configuration. The design assumes no, and states
  the limit.
  **ANSWERED (Sven, 2026-10-01): no. `ts/apps/*/moon.yml` stays off the filter; the docs state
  the limit.**
- **Q-a.** Is it acceptable that every `images.yml` run, rs-only PRs included, now pays for a
  full TS install and a host Next build of both consoles on each leg? The alternative is a
  condition on the step, which adds a second place that must agree with the filter. The design
  assumes yes.
  **ANSWERED (Sven, 2026-10-01): yes, accepted.**
- **Q-c.** (for the plan, not for Sven) Is the 6 s of "Build + smoke both consoles" on arm64 (F7)
  a BuildKit cache hit on the "Console release sequence" layers? The plan reads the log; D3
  records the answer. **Still open, for the plan (not a question for Sven).**

## 11. Challenge changelog

**Verdict of the spec-challenger:** APPROVE WITH CHANGES. No blocker, one major, thirteen minor,
four questions. I checked each finding against `origin/main` `83d446fc`. All findings were
justified, and I folded all of them.

**Folded:**

- MAJOR, the gate can switch off silently: D1 now has a strict parse of
  `${CONSOLE_PARITY_REQUIRED:-}` (0/empty/unset, 1, else error) and a workflow pin, row W1, with
  four mutations (§4.3). Verified: only step `env` turns the check on, and `set -u` is on in
  run.sh line 21 and in `run_fn`.
- `check_row` reads stderr only: `check_row` and `run_fn` get an optional stdout argument
  (§4.3). Verified at console-selftest.sh lines 84-106; the two lines go to stdout.
- No argv pin on the parity rows: new `$T/argv-parity`, used by each docker-reaching row.
- Missing arms: rows R1b, R2b, R3b, R4b, R4c, R4d, R5b, R5c and R5d added. Each row sets the
  variable itself.
- The "byte-for-byte" extraction: replaced by `local rc=0`, locals moved, `return "$rc"` (§4.1).
  Verified: the block writes `ec=1` and uses the locals from lines 1394-1396.
- `CONSOLE_STAGED_TREE_JS`: decided. It moves into the function as a `local`; the harness header
  is updated. Verified: only line 1713 uses it.
- The D2 gap claim was wrong: F6 corrected, old §4.4 (the `/gateway` tests) and old AC 7
  removed. Verified: `standalone-runtime.test.ts` lines 121-127 request `/gateway/healthz` from
  the built server, and `gateway-console-ts:test` depends on `~:build`.
- chart.yml reports the failure as infrastructure: D2 and §4.4 now name the unit and
  `standalone-runtime` tests as the primary control and chart.yml as the second.
- moon.yml-side drift: stated in D1, R-e, §4.4 and `rs/CLAUDE.md`; Q5 added. Verified:
  `ts/apps/*/moon.yml` is not on the `images.yml` filter.
- The misleading "DIFFERENT sources" message: a required-mode text that names the chunk-name
  assumption, a RUNBOOK response and the rollback (§4.1, §4.4), row R4b.
- R-c checked the wrong thing: replaced by a Docker-context check with `--ignored` (§4.2, AC 9),
  and F13 corrected. Verified locally: `tsconfig.tsbuildinfo` and `next-env.d.ts` are ignored
  files that `ts/.dockerignore` does not exclude.
- R-b priced only pnpm and Next: R-b now includes moon's cold start; the step gets
  `timeout-minutes`; the plan confirms moon's frozen install or keeps the explicit line; Q-a
  added.
- A parity miss stops the second console: the loop now keeps an rc per key (§4.2, AC 10).
  Verified at images.yml lines 186-198.
- D3 pointed to a home that does not exist: the figures go to the PR description and a Linear
  comment; the amd64 disk size is added. Verified: the RUNBOOK holds no job figures.
- The Q2 cache key would collide: the key rule is in §4.2 and Q2. Verified: chart.yml line 112.
- Questions: the 6 s cache-hit question is Q-c and part of D3. The R-a-first question is now §6
  "Measure R-a first". The rs-only cost question is Q-a.

**Rejected:** none.

Also corrected while checking: the "platform split" risk is in RUNBOOK section 6, not section 5.

## 12. Corrections of 2026-10-01 (re-check against `origin/main` `ecc87bbd`)

The setup agent applied the approval decisions (§0, §10) and checked each cited path, line and
assumption against `ecc87bbd`. SMA-675 (`52d51f82`, PR 339) is the only commit since `83d446fc`
that changed the files of this spec. Changes:

1. **Design change, the switch is an argument (F16).** SMA-675 recorded the rule that a dispatch
   helper reads a smoke switch and passes it to `smoke_consoles` as an argument, and that no row
   function reads the variable. The 2026-09-27 design had the row read `CONSOLE_PARITY_REQUIRED`
   itself. D1 and §4.1 now add `parity_required_flag`, a second argument
   `--parity=required|optional` to `smoke_consoles`, and a third argument to the row. The
   variable name, its values and the strict parse are unchanged. Rejected-alternatives list
   updated.
2. **W1 excluded the cargo smoke step (F20).** "Every step that calls `ci/images/run.sh smoke`"
   also matched "Smoke each service on its own" (`smoke gateway`, `smoke iam`), which must not
   need the line. W1 now defines a console smoke step and has a fifth mutation for it.
3. **Row names (F18).** `R0`-`R4` are the SMA-675 Redis rows. The parity rows are now `SP1`-`SP5d`.
   New rows: `PF0`-`PF2` (helper), `Z6` (second argument), `D2` (real dispatch).
4. **No `check_row` change (F14).** `expect_in` and `expect_not_in` already check stdout. The
   planned eighth argument of `check_row` and `run_fn` is dropped.
5. **Harness details (F14, F21).** Rows set `FX_ROOT`, not `ROOT`. `smoke_case` sets
   `CONSOLE_STAGED_TREE_JS=""` today and must drop that line, and it stubs the new row. The
   harness unsets `CONSOLE_PARITY_REQUIRED` once at the top. The harness header lines 14-15 are
   already stale since SMA-675.
6. **A PR run has the amd64 leg only (F17).** AC 1 said "both legs" on a PR. AC 1, D1, §4.4 and
   §6 now say: amd64 on a PR, both legs on `main` and on a dispatch.
7. **The three filter lists do not agree (F19).** D2 said they agree; `rs/CLAUDE.md` lacks the
   four SMA-675 smoke-runtime entries. §4.4 and AC 6 now include that correction.
8. **Docker-context check scope.** It now also covers `rs/crates/bindings`, the SMA-634 named
   context, which has no `.dockerignore` (§4.2, AC 9).
9. **D3 and R-b figures.** Run 36347690901 predates SMA-675's Redis sidecar and kernel rows; the
   figures must say so, and the plan reads a current `main` run for the base job time. §4.2
   notes that the `ci.yml` 30-minute limit does not apply to `images.yml`.
10. **Line numbers and paths updated:** the parity block 1694-1784 → 2157-2247; the
    `smoke_consoles` locals 1394-1396 → 1809-1811; `CONSOLE_STAGED_TREE_JS` use 1713 → 2176
    (definition 1601-1621); section contract line 1211 → 1626; the shape-only comment
    1749-1759 → 2207-2221; the "DIFFERENT sources" arm 1764 → 2227; the "no host build" comment
    1774-1782 → 2236-2245; `build_console` 622-633 → `build_console_one` 625-637; `images.yml`
    callers 192/211 → 203/222; the release-sequence loop 186-198 → 195-209; `release.yml` smoke
    line 1246 → 1252; RUNBOOK section 6 lines 440-447 → 486-491 and 448-455 → 492-500; RUNBOOK
    kind rc 2 lines 462-464 → 506-508; `argv-config` line 567 → 728; `run_fn` line 492 →
    636-667; `standalone-runtime.test.ts` lines 121-127 → 121-132; `canonicalBasePath` now
    takes two arguments (`base-path.test.ts` line 7). Unchanged and confirmed: `base_path_for`
    line 581, `ts/Dockerfile` line 55, `check_row` lines 84-106, gateway-console `moon.yml` line
    114, `chart.yml` lines 98-102, 104-114 and 112, `ts/.dockerignore` content.
11. **Branch name** in the header: `feature/sma-671-images-console-coverage`.
