# SMA-389 — Wire `paigasus-proto` build → `contracts:generate` edges with the first real proto

**Status:** approved design
**Linear:** [SMA-389](https://linear.app/smaschek/issue/SMA-389/wire-paigasus-proto-build-contractsgenerate-dependency-edges-when)
**Date:** 2026-06-13
**ADR:** ADR-0004 (Protobuf + buf as the single source of truth for wire contracts), ADR-0005 (kernel-once)
**Follows:** SMA-360 (§8 / finding H3 deferred this wiring to "land with the first real protos")

## Goal

Land the first genuine `.proto` schema and wire the build-graph edges so the
headline monorepo behavior works end-to-end:

> touch a proto → `contracts:generate` → `paigasus-proto:{build,test}` → the
> correct affected downstream rebuilds, via `moon ci`.

SMA-360 deliberately deferred these edges: with zero protos, `contracts:generate`
is a no-op and wiring it would force `buf` onto PATH for every proto build for no
benefit. This issue resolves that deferral by landing a real schema *together
with* the edges, committing the generated code (ADR-0004), and keeping generated
code out of the strict lint/format gates.

## Decisions resolved during brainstorming

1. **Author one genuine schema, not just wire edges.** AC #3 ("a proto edit
   triggers regeneration + the correct downstream rebuilds") is only meaningful
   with consumable generated code. We land one small but real proto rather than
   wiring against an empty `generate`.
2. **First proto = a gRPC `HealthService`** (`gateway/v1/health.proto`), chosen
   over a messages-only `common/v1` type because it exercises **both** prost
   (messages) and tonic (service stubs) — the fuller, riskier Rust codegen path —
   and matches the existing `gateway/v1/.gitkeep` intent.
3. **Downstream proof = in-package smoke tests + the existing affected graph.**
   Each proto package gets a tiny test that imports its generated types, so the
   code is genuinely compiled and exercised. True downstream consumers (e.g.
   `paigasus-gateway-rs`) re-run via Moon's existing project-graph affected
   detection. Real gateway *consumption* of the service stays in the gateway
   issue — out of scope here.
4. **Edge mechanism = task-level `deps: ['contracts:generate']`** on each
   consuming task. This both orders `buf generate` before the build *and* makes
   Moon treat `contracts` as a project-dependency of each proto package, so
   affected-detection propagates (`contracts → proto-rs → gateway`). Rejected:
   adding global `^:build` to the language `build` tasks (broader DAG change,
   out of scope) and project-level `dependsOn: contracts` (contracts exposes no
   `build` task for `^:build` to bind to).
5. **rustfmt ignores the generated dir** (`ignore = ["src/generated"]`): prost's
   prettyplease output is not byte-identical to rustfmt, so `cargo fmt --check`
   would otherwise fail on generated code.
6. **The Python whole-tree `typecheck`/`test` also get the edge.** Those tasks
   live on the `py` root project (SMA-401 whole-tree dedup) yet genuinely consume
   generated code, so the rigorous ordering edge belongs there too — accepting
   that this forces `buf` onto PATH for every py-root check. `ruff`/`prettier`
   and the ts/rust whole-tree lints do **not** get the edge because generated
   code is lint-excluded (below), so they consume nothing.
7. **Remove `common/v1/reserved.proto`.** Its own comment marks it a placeholder
   to be replaced once contracts work begins; `health.proto` keeps the buf module
   non-empty.

## 1. Proto source

`contracts/proto/paigasus/gateway/v1/health.proto`:

```proto
// SPDX-License-Identifier: Apache-2.0
syntax = "proto3";

package paigasus.gateway.v1;

// Minimal liveness probe — the first real contract. Exercises the full
// prost + tonic (and betterproto2 / protobuf-es) codegen path end-to-end.
service HealthService {                                   // SERVICE_SUFFIX
  rpc Check(CheckRequest) returns (CheckResponse);        // RPC_*_STANDARD_NAME
}

message CheckRequest {}

message CheckResponse {
  string status = 1;
}
```

Names satisfy buf `STANDARD` lint (the repo's posture, minus
`PACKAGE_DIRECTORY_MATCH`): service suffixed `Service`, request/response named
`{Rpc}Request`/`{Rpc}Response`, field `lower_snake_case`, package versioned.

Delete `contracts/proto/paigasus/common/v1/reserved.proto`.

## 2. buf config

- `contracts/buf.gen.yaml`: add **`clean: true`** (SMA-360 §3 deferred this to the
  PR that lands the first protos). With committed codegen, `clean` wipes each
  `out:` dir before regeneration so stale files can't linger.
- Delete the four `generated/.gitkeep` stubs (`clean` would remove them anyway;
  real generated files now occupy those dirs).

## 3. Generated-code landing zones (committed, ADR-0004)

`buf generate` writes, per the existing `buf.gen.yaml` plugin/opt set:

| Lang | Plugin(s) | Out dir |
|------|-----------|---------|
| Rust | neoeinstein-prost + neoeinstein-tonic | `rs/crates/libs/paigasus-proto/src/generated` |
| Py   | danielgtaylor-betterproto (betterproto2) | `py/packages/paigasus-proto/src/paigasus_proto/generated` |
| TS   | bufbuild-es (protobuf-es v2) | `ts/packages/paigasus-proto/src/generated` |

All generated files are committed.

## 4. Rust crate (`paigasus-proto`)

- Add `prost` and `tonic` to `[workspace.dependencies]` and consume them in the
  proto crate. **Versions must match the neoeinstein-prost/tonic remote-plugin
  output** — plugin↔crate version skew is the classic prost/tonic footgun;
  verify a clean `cargo build`. (`bytes` is already a workspace dep, satisfying
  the prost `bytes=.` opt.)
- `src/lib.rs`: declare a `gateway::v1` module that `include!`s the generated
  package file, annotated `#[allow(clippy::all)]` so generated code stays out of
  the `clippy -D warnings` gate. The `file_descriptor_set` `.bin` artifact is a
  committed data file (not `mod`-included).
- `rs/crates/libs/paigasus-proto/rustfmt.toml`: `ignore = ["src/generated"]`.

## 5. Build-graph edges

Add task-level `deps` (Moon merges these onto the inherited tasks):

| Project / task | New dep | Rationale |
|----------------|---------|-----------|
| `paigasus-proto-rs:build`, `:test` | `contracts:generate` | AC #1; both compile generated code |
| `paigasus-proto-py:build` | `contracts:generate` | AC #2; `uv build` packages generated code |
| `paigasus-proto-ts:build`, `:typecheck`, `:test` | `contracts:generate` | AC #2; tsc/vitest compile `src/generated` |
| `py:typecheck`, `py:test` | `contracts:generate` | decision #6; whole-tree basedpyright/pytest consume generated code |

The rs/ts proto packages own their per-package build/typecheck/test (library
layer), so the edge sits on the package. Python's whole-tree typecheck/test live
on the `py` root, hence the extra two rows.

## 6. Lint / format integration (keep generated code out of the gates)

- **Rust:** `#[allow(clippy::all)]` on the generated module (clippy) + rustfmt
  `ignore` (fmt) — see §4.
- **Python:** add `**/generated/**` to `tool.ruff.extend-exclude` *and*
  `tool.basedpyright.exclude` in `py/pyproject.toml`. Excluded files stay
  import-resolvable (so the smoke test still gets types), they're just not
  error-reported.
- **TS:** add `**/generated/**` to the eslint `ignores` array in
  `ts/eslint.config.js` and to `ts/.prettierignore`. `tsc` still compiles
  generated code — that *is* the typecheck.

## 7. Smoke tests (in-package, exercise generated types)

One minimal test per package, so generated code is compiled and run:

- **Rust** (`paigasus-proto-rs:test`): construct `CheckResponse { status: … }`,
  assert the field round-trips. (Message-only — no tonic server/tokio runtime
  needed; the tonic service stubs are still compiled by `build`.)
- **Python** (root `pytest`, `py/packages/paigasus-proto/tests/`): import the
  generated module, construct `CheckResponse`, assert.
- **TS** (`paigasus-proto-ts:test`, vitest): import from `src/generated`,
  construct the message, assert.

## 8. Data flow — how AC #3 is satisfied

```
edit contracts/proto/paigasus/gateway/v1/health.proto
        │  (file under contracts project)
        ▼
contracts  ── affected ──▶  paigasus-proto-{rs,py,ts}   (new task-dep edge)
                                   │
                                   ▼  (existing dependsOn)
                            paigasus-gateway-rs           ── affected
```

Within a `moon ci :build` / `:test` run, the ordering edge forces
`contracts:generate` to run first (regenerating committed code); the proto
packages then build/test against fresh code, and affected consumers re-run.
`contracts:generate` runs **once** per invocation regardless of how many
dependents reference it (Moon dedups).

## 9. Risks & caveats

- **buf-on-PATH in CI (SMA-360 caveat M3, now due).** The edge forces `buf` onto
  PATH for every affected proto build *and* every py-root check. `ci.yml`
  (landed since SMA-360, SMA-361/363) must activate proto's shims **before**
  `moon ci`. Verification includes a clean, rc-free shell check (CI proxy).
- **prost/tonic plugin↔crate version skew.** Pin crate versions to the remote
  plugin output; verify clean `cargo build` + smoke test.
- **`contracts:generate` declares no `outputs`.** Its outputs land in sibling
  projects (`../rs`, `../py`, `../ts`) — outside the contracts project root, which
  Moon cannot list as task outputs. The mechanism is therefore the ordering edge
  plus the committed generated code (an input of each consumer), not output
  caching/hydration of `generate`. Accepted.
- **Broader py blast radius (decision #6).** Every `py:test`/`py:typecheck` now
  depends on `contracts:generate`; acceptable given buf is on PATH in CI and
  after `proto install` locally.

## Verification (maps to acceptance criteria)

1. `moon run contracts:lint` and `:generate` succeed; generated Rust/Py/TS files
   appear under the three landing zones and are committed; `.gitkeep` stubs gone.
2. **AC #1** — `paigasus-proto-rs:build` and `:test` list `contracts:generate` in
   their dep graph (`moon project paigasus-proto-rs`); `cargo build -p
   paigasus-proto` + `cargo nextest` (smoke) green; `clippy -D warnings` and
   `fmt --check` clean.
3. **AC #2** — `paigasus-proto-py:build` and `paigasus-proto-ts:{build,typecheck,
   test}` depend on `contracts:generate`; `uv build`, root `pytest`/`basedpyright`,
   `tsc`/`vitest`/`eslint`/`prettier` all green with generated code excluded from
   the linters.
4. **AC #3** — edit `health.proto`, run `moon ci :build --base main` (and `:test`);
   confirm `contracts` → `paigasus-proto-{rs,py,ts}` → `paigasus-gateway-rs` all
   re-run in the affected set, in that order.
5. Clean, shell-rc-free shell resolves `buf` (CI proxy) before `moon ci`.

## Out of scope

- Real `paigasus-gateway` *implementation* / consumption of `HealthService` → its
  own issue.
- `codegen-drift.yml` nightly CI (committed-codegen drift guard) → follow-up.
- Flipping `paigasus-proto` `publish=false` / ts `private` → **SMA-388** (which
  this unblocks).
- Any further domain schemas beyond the single `HealthService`.

## Follow-ups

- **SMA-388** — flip `paigasus-proto` `publish`/`private` once generated code
  lands (this issue unblocks it).
- Nightly codegen-drift CI (re-affirm committed generated code matches protos).
