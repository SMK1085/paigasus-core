# affected-graph regression guard (SMA-409)

`moon ci` *uses* the affected graph but never *asserts* it is correct: a deleted
`dependsOn` edge — or a dropped `moon ci --include-relations` — makes the affected set
silently shrink, so CI under-builds and stays **green**. This guard closes that gap.

`run.sh` feeds a synthetic touched-file to `moon query projects --affected --downstream
deep` and asserts the affected project set per known case (`repo`, which owns the whole
tree as its source, is filtered out):

- **contracts edit** → `contracts` + `paigasus-proto-{rs,py,ts}` + `paigasus-gateway-rs`.
- **kernel edit** → `paigasus-kernel-rs` + `paigasus-py-bindings-rs` + `paigasus-node-bindings-rs`
  + `paigasus-gateway-rs` + `paigasus-kernel-py` + `paigasus-kernel-ts` (both language wrappers now
  wrap their bindings, SMA-419/420); still **no `contracts` / unrelated `*-py`
  (`paigasus-proto/workflows/ml-py`) / unrelated `*-ts`** (`paigasus-proto/sdk/ui/console/docs-ts`,
  `commitlint-config-ts`).
- **py binding edit** → `paigasus-py-bindings-rs` + `paigasus-kernel-py`; one-directional w.r.t.
  the kernel.
- **node binding edit** → `paigasus-node-bindings-rs` + `paigasus-kernel-ts`; one-directional
  w.r.t. the kernel.

It also asserts every `moon ci` invocation in `.github/workflows/ci.yml` carries
`--include-relations` (the edges are inert without it).

Run locally: `moon run repo:affected-smoke` (or `ci/affected-graph/run.sh`).
Prove it can fail: `ci/affected-graph/run.sh --negative-control`.

## Maintenance — the must-exclude assertions are topology-coupled (SMA-409 F5)

The **must-include** sets are durable. The **must-exclude** (cross-stack-isolation) assertions
track current topology. Both the **py** and **ts** kernel-wrapper edges have now landed
(SMA-419/420). The `kernel->bindings` forbid-regex enumerates the *unrelated* ts/py packages a
kernel edit must not reach; each newly-added ts/py package must be hand-added to that enumeration
or it is silently unasserted. Consolidating this into a completeness/default-deny meta-check is a
tracked follow-up (SMA-420 review F4) — it would reverse the deliberate "positive-superset, not
strict equality" choice (SMA-409), so it gets its own decision.
