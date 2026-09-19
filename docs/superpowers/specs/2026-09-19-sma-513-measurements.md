# SMA-513 — Measurements

This file records point-in-time measurements taken while implementing the SMA-513 plan
(`docs/superpowers/plans/2026-09-19-sma-513-console-images-and-chart.md`). Each section is
numbered by the task that produced it and states the exact command, the result, and the date.

<!-- M1 is added by Task 1 (the filtered pnpm install measurement). Not written here. -->

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
