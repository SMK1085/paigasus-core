# affected-graph regression guard (SMA-409)

`moon ci` *uses* the affected graph but never *asserts* it is correct: a deleted
`dependsOn` edge — or a dropped `moon ci --include-relations` — makes the affected set
silently shrink, so CI under-builds and stays **green**. This guard closes that gap.

`run.sh` feeds a synthetic touched-file to `moon query projects --affected --downstream
deep` and asserts the affected project set per known case (`repo`, which owns the whole
tree as its source, is filtered out):

- **contracts edit** → `contracts` + `paigasus-proto-{rs,py,ts}` + `paigasus-gateway-rs`.
- **kernel edit** → `paigasus-kernel-rs` + `paigasus-py-bindings-rs` + `paigasus-gateway-rs`,
  and **nothing cross-stack** (no `*-py` / `*-ts` / `contracts`).
- **binding edit** → only `paigasus-py-bindings-rs` (the edge is one-directional).

It also asserts every `moon ci` invocation in `.github/workflows/ci.yml` carries
`--include-relations` (the edges are inert without it).

Run locally: `moon run repo:affected-smoke` (or `ci/affected-graph/run.sh`).
Prove it can fail: `ci/affected-graph/run.sh --negative-control`.

## Maintenance — the must-exclude assertions are topology-coupled (SMA-409 F5)

The **must-include** sets are durable. The **must-exclude** (cross-stack-isolation)
assertions hold only because the py/ts kernel wrappers are deferred. When the deferred
uv↔maturin integration lands and `paigasus-kernel-py` genuinely wraps the wheel, a kernel
edit *should* affect the py wrapper — and the `kernel->bindings` forbid-regex here will
correctly need loosening. A failure there is the expected next edge, not a regression;
update this guard alongside each deferred binding.
