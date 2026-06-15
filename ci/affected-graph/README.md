# affected-graph regression guard (SMA-409)

`moon ci` *uses* the affected graph but never *asserts* it is correct: a deleted
`dependsOn` edge — or a dropped `moon ci --include-relations` — makes the affected set
silently shrink, so CI under-builds and stays **green**. This guard closes that gap.

`run.sh` feeds a synthetic touched-file to `moon query projects --affected --downstream
deep` and asserts the affected project set per known case (`repo`, which owns the whole
tree as its source, is filtered out):

- **contracts edit** → `contracts` + `paigasus-proto-{rs,py,ts}` + `paigasus-gateway-rs`.
- **kernel edit** → `paigasus-kernel-rs` + `paigasus-py-bindings-rs` + `paigasus-gateway-rs`
  + `paigasus-kernel-py` (the py wrapper now wraps the wheel, SMA-419); still **no `*-ts` /
  `contracts` / unrelated `*-py`** (`paigasus-proto/workflows/ml-py`).
- **binding edit** → `paigasus-py-bindings-rs` + `paigasus-kernel-py`; still one-directional
  w.r.t. the kernel (never drags in `paigasus-kernel-rs`).

It also asserts every `moon ci` invocation in `.github/workflows/ci.yml` carries
`--include-relations` (the edges are inert without it).

Run locally: `moon run repo:affected-smoke` (or `ci/affected-graph/run.sh`).
Prove it can fail: `ci/affected-graph/run.sh --negative-control`.

## Maintenance — the must-exclude assertions are topology-coupled (SMA-409 F5)

The **must-include** sets are durable. The **must-exclude** (cross-stack-isolation)
assertions track current topology. The **py** wrapper edge landed in SMA-419
(`paigasus-kernel-py` moved from forbid → must-include). The remaining deferred edge is the
**ts** kernel wrapper: when it lands, a kernel edit *should* affect it, and the
`kernel->bindings` forbid-regex here will correctly need its `-ts$` term loosened. A failure
there is the expected next edge, not a regression; update this guard alongside that work.
