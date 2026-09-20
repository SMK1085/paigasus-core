# SMA-513 — Measurements

This file records point-in-time measurements taken while implementing the SMA-513 plan
(`docs/superpowers/plans/2026-09-19-sma-513-console-images-and-chart.md`). Each section is
numbered by the task that produced it and states the exact command, the result, and the date.

## M1 — filtered pnpm install and the kernel packages (Task 1)

**Date:** 2026-09-20

**pnpm version:** `11.3.0` (measured with `pnpm --version` under the proto shim). This is a
resolver behaviour; re-measure on any pnpm bump.

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

No `node_modules` or `.pnpm` directory was created under `$SCRATCH` at all — not even an empty
one. `ls "$SCRATCH/node_modules/@paigasus"` printed `(no @paigasus dir)`. Re-running the same
command with `--force` added produced the identical `Already up to date` result and still
created nothing under `$SCRATCH`.

**This result does not answer the question.** pnpm's "up to date" short-circuit is decided
against `ts/node_modules/.pnpm-workspace-state-v1.json` (which already exists and is current in
this worktree, since the worktree was provisioned with a full `pnpm -C ts install
--frozen-lockfile`), not against the custom `--modules-dir`/`--virtual-store-dir` target. Because
pnpm judged the workspace already satisfied, it skipped all linking work and never populated the
scratch directories — so this command, run in an already-provisioned worktree, cannot show what a
filtered install would materialize into a fresh tree. This is a property of pnpm 11.3.0's
up-to-date check, not of the `--filter` expression itself.

**Supplementary measurement (to answer the actual question):** `ts/` (excluding `node_modules`
and `.git`) was `rsync`-copied to a scratch directory outside the repo, giving a tree with no
pre-existing `.pnpm-workspace-state-v1.json` anywhere. The identical filtered-install command was
then run from that copy (same flags, same lockfile, same pnpm version). This time pnpm did real
work (`Lockfile is up to date, resolution step is skipped` → `added 944`), and
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

Neither `@paigasus/kernel` nor `@paigasus/node-bindings` is present. Note also that there is no
`ts/packages/paigasus-node-bindings` directory in this tree at all — the brief's premise names a
package that does not exist under that name; the only kernel-adjacent package present is
`ts/packages/paigasus-kernel` (`@paigasus/kernel`).

This is corroborated by the dependency graph: `@paigasus/sdk` and `@paigasus/discovery` depend
only on `@paigasus/proto`; `@paigasus/auth` has no `@paigasus/*` dependencies;
`@paigasus/console-core` depends on `auth`, `discovery`, `sdk`; `@paigasus/app-shell` depends on
`auth`, `discovery`, `ui`, `next-config`. None of these, transitively, depend on
`@paigasus/kernel`.

**Verdict: a filtered `pnpm install` is sufficient.** `@paigasus/iam-console` does not pull in
`@paigasus/kernel` through `@paigasus/sdk`, `@paigasus/auth`, or `@paigasus/discovery` — none of
the three names it in their dependency trees. Task 2's Dockerfile does not need to exclude
`ts/packages/paigasus-kernel` from the build context on this account.

**What Task 2 must know:**
- Do not reuse the brief's exact scratch-install command as a smoke test inside a
  worktree/checkout that already has `ts/node_modules` provisioned — it will report
  `Already up to date` and prove nothing, because the up-to-date check reads
  `ts/node_modules/.pnpm-workspace-state-v1.json` regardless of `--modules-dir`. A real Docker
  build starts from a clean `COPY`, so it does not hit this short-circuit, but a *local
  pre-flight check* run against a provisioned tree will.
- Re-verify this measurement if pnpm is bumped past 11.3.0, or if `@paigasus/kernel` gains a new
  consumer among `sdk`, `auth`, `discovery`, `console-core`, `app-shell`, `ui`, or `next-config`.

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
