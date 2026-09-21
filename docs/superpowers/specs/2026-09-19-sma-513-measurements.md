# SMA-513 — Measurements

This file records point-in-time measurements taken while implementing the SMA-513 plan
(`docs/superpowers/plans/2026-09-19-sma-513-console-images-and-chart.md`). Each section is
numbered by the task that produced it and states the exact command, the result, and the date.

## M1 — filtered pnpm install and the kernel packages (Task 1)

**Date:** 2026-09-20

**pnpm version:** `11.3.0` (measured with `pnpm --version` under the proto shim). The result is a
property of the pnpm resolver. Measure it again after a pnpm bump.

**Command (as given in the Task 1 brief):**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-console-helm
SCRATCH="$(mktemp -d)"
LOG="$(mktemp)"
pnpm -C ts install --frozen-lockfile --filter @paigasus/iam-console... \
  --virtual-store-dir "$SCRATCH/.pnpm" --modules-dir "$SCRATCH/node_modules" >"$LOG" 2>&1
ls "$SCRATCH/node_modules/@paigasus" 2>/dev/null || echo "(no @paigasus dir)"
```

**Result of the exact command above:** exit code 0. Full log:

```
Scope: 9 of 13 workspace projects
Already up to date
Done in 139ms using pnpm v11.3.0
```

The command made no `node_modules` or `.pnpm` directory under `$SCRATCH`, not even an empty one.
`ls "$SCRATCH/node_modules/@paigasus"` printed `(no @paigasus dir)`. The same command with
`--force` gave the same `Already up to date` result. It also made nothing under `$SCRATCH`.

**This result does not answer the question.** pnpm decides "up to date" from
`ts/node_modules/.pnpm-workspace-state-v1.json`, not from the `--modules-dir` or
`--virtual-store-dir` target. That file exists and is current in this worktree, because a full
`pnpm -C ts install --frozen-lockfile` prepared the worktree. Thus pnpm found the workspace
satisfied, did no linking, and did not fill the scratch directories. In a prepared worktree, this
command cannot show what a filtered install puts into a new tree. This is a property of the
up-to-date check in pnpm 11.3.0, not of the `--filter` expression.

**Supplementary measurement (to answer the question):** `rsync` copied `ts/` (without
`node_modules` and `.git`) to a scratch directory outside the repo. That tree had no
`.pnpm-workspace-state-v1.json` in it. The same filtered-install command then ran in that copy,
with the same flags, the same lockfile and the same pnpm version. This time pnpm did the install
(`Lockfile is up to date, resolution step is skipped` → `added 944`).
`ls "$SCRATCH/node_modules/@paigasus"` listed:

```
app-shell
auth
commitlint-config
console-core
discovery
next-config
proto
sdk
ui
```

The list does not contain `@paigasus/kernel` or `@paigasus/node-bindings`. This tree has no
`ts/packages/paigasus-node-bindings` directory. Thus the brief names a package that does not exist
under that name. The only kernel package in the tree is `ts/packages/paigasus-kernel`
(`@paigasus/kernel`).

The dependency graph gives the same result. `@paigasus/sdk` and `@paigasus/discovery` depend only
on `@paigasus/proto`. `@paigasus/auth` has no `@paigasus/*` dependencies. `@paigasus/console-core`
depends on `auth`, `discovery` and `sdk`. `@paigasus/app-shell` depends on `auth`, `discovery`,
`ui` and `next-config`. None of these depends on `@paigasus/kernel`, directly or transitively.

**Verdict: a filtered `pnpm install` is sufficient.** `@paigasus/iam-console` does not get
`@paigasus/kernel` through `@paigasus/sdk`, `@paigasus/auth` or `@paigasus/discovery`. None of the
three has it in its dependency tree. Thus the Task 2 Dockerfile does not need to exclude
`ts/packages/paigasus-kernel` from the build context for this reason.

**What Task 2 must know:**
- Do not use the exact scratch-install command of the brief as a smoke test in a worktree or
  checkout that already has `ts/node_modules`. It reports `Already up to date` and proves nothing,
  because the up-to-date check reads `ts/node_modules/.pnpm-workspace-state-v1.json` and ignores
  `--modules-dir`. A real Docker build starts from a clean `COPY`, so this short-circuit does not
  occur there. A local check before the build, on a prepared tree, gets the short-circuit.
- Do this measurement again if pnpm moves past 11.3.0. Also do it again if `@paigasus/kernel`
  gets a new consumer among `sdk`, `auth`, `discovery`, `console-core`, `app-shell`, `ui` or
  `next-config`.

## M2 — helm version pinned through proto (Task 7)

**Date:** 2026-09-20

**Command:**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
proto install helm
helm version --short
```

**Result:** `v3.22.0+g144ca65`, resolved at `/Users/smaschek/.proto/shims/helm` (installed to
`/Users/smaschek/.proto/tools/helm/3.22.0/darwin-arm64/helm`). The plugin resolved and installed
on the first attempt — no URL template correction was needed.

**Why 3.x over 4.x:** Both major lines are maintained (latest 3.x is 3.22.0, latest overall is
4.3.0, also measured 2026-09-20). 3.x is chosen because PR 3's kind job installs ingress-nginx's
own chart, and that ecosystem targets helm 3. This repository's chart is `apiVersion: v2`, which
both helm 3 and helm 4 render, so nothing in the chart itself depends on this choice. Moving to
4.x later is a deliberate golden-file re-baseline, not a drop-in bump.
