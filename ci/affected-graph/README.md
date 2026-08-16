# affected-graph regression guard (SMA-409 / SMA-429)

`moon ci` *uses* the affected graph but never *asserts* it is correct: a deleted
`dependsOn` edge — or a dropped `moon ci --include-relations` — makes the affected set
silently shrink, so CI under-builds and stays **green**. This guard closes that gap.

`run.sh` feeds a synthetic touched-file to `moon query projects --affected --downstream
deep` and asserts the affected project set **equals** an exact expected set per known case
(default-deny; `repo`, which owns the whole tree as its source, is filtered out):

- **contracts edit** → `contracts` + `paigasus-proto-{rs,py,ts}` + `paigasus-gateway-rs`
  + `paigasus-iam-rs` (SMA-442) + `paigasus-service-info-rs` (SMA-505).
- **derive-crate edit** → `paigasus-proto-derive-rs` + `paigasus-proto-rs` + `paigasus-gateway-rs`
  + `paigasus-iam-rs` + `paigasus-service-info-rs` (SMA-438/SMA-524). One-directional w.r.t.
  contracts: the derive crate is strictly upstream of `paigasus-proto`.
- **service-info edit** → `paigasus-service-info-rs` + `paigasus-iam-rs` + `paigasus-gateway-rs`
  (SMA-524). One-directional w.r.t. `paigasus-proto`.
- **kernel edit** → `paigasus-kernel-rs` + `paigasus-py-bindings-rs` + `paigasus-node-bindings-rs`
  + `paigasus-wasm-rs` + `paigasus-gateway-rs` + `paigasus-kernel-py` + `paigasus-kernel-ts`
  + `paigasus-kernel-parity-rs` (both language wrappers wrap their bindings, SMA-419/420/427)
  + `paigasus-iam-core-rs` + `paigasus-iam-rs` (SMA-441).
  Strict equality rejects any other project implicitly.
- **py binding edit** → `paigasus-py-bindings-rs` + `paigasus-kernel-py`; one-directional w.r.t.
  the kernel.
- **node binding edit** → `paigasus-node-bindings-rs` + `paigasus-kernel-ts`; one-directional
  w.r.t. the kernel.
- **wasm binding edit** → `paigasus-wasm-rs` + `paigasus-kernel-ts`; one-directional w.r.t. the
  kernel. `paigasus-kernel-ts` now has two upstream binding edges — `paigasus-kernel-rs →
  paigasus-node-bindings-rs → paigasus-kernel-ts` (napi) and `paigasus-kernel-rs →
  paigasus-wasm-rs → paigasus-kernel-ts` (wasm, SMA-427) — so a kernel edit reaches it via both.
- **parity-crate edit** → `paigasus-kernel-parity-rs`; one-directional w.r.t. the kernel (a parity
  edit must not rebuild the kernel). The py/ts parity tests list the corpus as a task `input`
  (cache-keying), which does not make them project-affected by a corpus-only edit.

It also runs two checks that the per-case project sets structurally **cannot** make:

- **`proto->service-info-tasks`** asserts the affected *task* set (`moon query tasks --affected`),
  scoped to `build`, `test` and `lint` — the three tasks that carry `^:build`. `moon query projects
  --affected` follows `dependsOn` only and is blind to a task-level `^:build`, so deleting one keeps
  every project case **green** while `moon ci --include-relations` silently under-builds (SMA-429
  F3, closed for build/test by SMA-524 and for lint by SMA-526). `lint`'s `^:build` is declared once,
  in `.moon/tasks/rust.yml`, rather than per-crate the way build/test declare theirs — so this case
  is also what catches a regression in that shared declaration.
- **`cargo-moon-parity`** (`cargo_moon_parity.py`) compares every crate's Cargo deps against Moon's own
  resolved graph, asserting each edge exists *and* schedules the upstream's build. The per-case sets
  assert only edges someone remembered to write a case for; this catches a crate added with **no**
  case — which is how SMA-524's bug survived a full review cycle. Edges intentionally declared without
  Cargo backing live in its `ALLOW_NO_CARGO_BACKING` table with a required reason string.

It also asserts every `moon ci` invocation in `.github/workflows/ci.yml` carries
`--include-relations` (the edges are inert without it).

Run locally: `moon run repo:affected-smoke` (or `ci/affected-graph/run.sh`).
Prove it can fail: `ci/affected-graph/run.sh --negative-control`.

## Maintenance — expected sets are exact (default-deny, SMA-429)

Each case asserts the affected set (minus `repo`) **equals** its expected set exactly — there is
no separate must-exclude list and no forbid enumeration. Cross-stack isolation is enforced
implicitly: any project that appears but isn't in the expected set fails the case.

- A project **unrelated** to a case never enters its downstream set, so it never appears → no
  maintenance (this is what the old hand-maintained forbid-regex existed to track).
- A project that **legitimately** becomes a new dependent (e.g. a future wasm kernel binding)
  makes the case fail with an `unexpected` entry → confirm the new edge is intended, then add the
  one project to that case's expected set.
- A **task** case (`assert_task_case`, e.g. `proto->service-info-tasks`) works at `pid:task`
  granularity, not project granularity, so its set can also grow without any new dependent
  project: widening the task-name filter itself (e.g. `lint` joining `build`/`test` in SMA-526)
  makes every already-listed project pick up a new `pid:task` row at once → same fix, confirm
  the new rows are intended, then add them to the case's expected set.

The expected sets are a snapshot of `moon query --affected --downstream deep` output at the
**pinned moon version** (currently 2.3.2). A moon upgrade that changes the affected-set output —
even benignly — will fail the guard, so re-grounding the expected sets is a known step of any
moon bump.
