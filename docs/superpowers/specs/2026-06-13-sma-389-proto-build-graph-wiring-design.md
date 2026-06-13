# SMA-389 — Wire `paigasus-proto` build → `contracts:generate` edges with the first real proto

**Status:** approved design (revised post staff-engineer review)
**Linear:** [SMA-389](https://linear.app/smaschek/issue/SMA-389/wire-paigasus-proto-build-contractsgenerate-dependency-edges-when)
**Date:** 2026-06-13
**ADR:** ADR-0004 (Protobuf + buf as the single source of truth for wire contracts), ADR-0005 (kernel-once)
**Follows:** SMA-360 (§8 / finding H3 deferred this wiring to "land with the first real protos"); SMA-374 (slimmed the rust template, explicitly deferred the codegen edges to here, and removed an earlier `proto-rs:build → contracts:generate` draft — **this issue supersedes that deferred wiring**)

## Goal

Land the first genuine `.proto` schema and wire the build-graph edges so the
headline monorepo behavior works end-to-end:

> touch a proto → `contracts:generate` → `paigasus-proto:{build,test}` → the
> correct affected downstream rebuilds, via `moon ci`.

SMA-360 deliberately deferred these edges: with zero protos, `contracts:generate`
is a no-op and wiring it would force `buf` onto PATH for every proto build for no
benefit. This issue resolves that deferral by landing a real schema *together
with* the edges, committing the generated code (ADR-0004), pinning the codegen
toolchain for determinism, and keeping generated code out of the strict
lint/format gates.

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
   out of scope, and correctly *not* landed by SMA-374) and project-level
   `dependsOn: contracts` (contracts exposes no `build` task for `^:build` to
   bind to).
5. **rustfmt ignores the generated dir** (`ignore = ["src/generated"]`): prost's
   prettyplease output is not byte-identical to rustfmt, so `cargo fmt --check`
   would otherwise fail on generated code.
6. **The Python whole-tree `typecheck`/`test` do *not* get the edge** (reverses an
   earlier draft). They live on the `py` root (SMA-401
   whole-tree dedup) and run on *any* py change. Decisions #8 (pinned plugins) +
   #9 (PR drift gate) make committed generated code byte-identical to regenerated
   code, so the whole-tree py checks read the committed code safely and need no
   pre-generate ordering — the drift gate is the backstop. This keeps codegen off
   the hot path of proto-unrelated py work. (`ruff`/`prettier` and the ts/rust
   whole-tree lints likewise get no edge: generated code is lint-excluded, so they
   consume nothing.)
7. **Remove `common/v1/reserved.proto`.** Its own comment marks it a placeholder
   to be replaced once contracts work begins; `health.proto` keeps the buf module
   non-empty.
8. **Pin all four remote codegen plugins** in `buf.gen.yaml` and pin the
   prost/tonic crates to match. Untagged plugins resolve *latest* at generate
   time; with committed codegen + `clean: true` +
   build→generate that floats the output, makes the crate↔plugin pin meaningless,
   and would false-positive any drift check on every upstream plugin release.
9. **Land a PR-level codegen drift gate now**, not a deferred nightly. `clean:
   true` + build→generate means CI compiles *regenerated*
   output, so stale committed codegen could merge uncaught. The build graph
   already regenerates, so a `git diff --exit-code` on the generated dirs is
   nearly free. Sound only because of #8 (deterministic regeneration).

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
- **Pin every remote plugin to an explicit version** (decision #8):
  `remote: buf.build/community/neoeinstein-prost:vX.Y.Z` (and `…-tonic`,
  `…danielgtaylor-betterproto`, `bufbuild/es`). Exact versions chosen at
  implementation time; they fix the prost/tonic crate pins in §4.
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
  proto crate. **Versions must match the *pinned* plugin versions** (§2 /
  decision #8) — plugin↔crate version skew is the classic prost/tonic footgun;
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

The rs/ts proto packages own their per-package build/typecheck/test (library
layer), so the edge sits on the package. Python's whole-tree typecheck/test live
on the `py` root and deliberately carry **no** edge (decision #6): committed code
is authoritative (decisions #8/#9), so they read it directly without forcing
codegen onto every py change.

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
dependents reference it (Moon dedups), and is cache-skipped entirely when the
proto inputs are unchanged.

## 9. Codegen determinism & PR drift gate

Committed codegen + `clean: true` + build→generate only behaves if regeneration
is **deterministic** and **verified**:

- **Determinism (decision #8).** All four remote plugins are version-pinned in
  `buf.gen.yaml`, and the prost/tonic crates are pinned to match. Without this,
  `buf generate` floats to latest: CI compiles a moving target, the crate pin
  desyncs on the next upstream plugin release (with no `.proto` change), and any
  drift check false-positives on plugin churn.
- **Drift gate (decision #9).** A PR-level CI step regenerates and runs
  `git diff --exit-code` over the three generated dirs. Because `moon ci`'s build
  graph already runs `contracts:generate`, the marginal cost is one `git diff`.
  - *Pass* → committed code matches the protos, and (given pinning) the bytes CI
    builds equal the bytes a reviewer approved.
  - *Fail* → a PR edited a `.proto` without regenerating-and-committing; caught at
    PR time instead of silently merging (CI never commits its own regeneration).

This pulls the SMA-360 "codegen-drift" guard forward from a deferred nightly into
a cheap PR gate; a nightly re-affirmation can still follow (see Follow-ups).

## 10. Risks & caveats

- **buf-on-PATH in CI — already mitigated (was SMA-360 caveat M3).** `ci.yml` runs
  `proto install` → `moon setup` before `moon ci :lint :breaking …`, and both
  `contracts:lint` and `contracts:breaking` are buf commands — so buf-on-PATH is
  proven in CI today. The new edges add more buf calls on an already-working PATH.
  Verification keeps a clean, rc-free shell check as a guard, but the risk is low.
- **`contracts:generate` declares no `outputs`.** Its outputs land in sibling
  projects (`../rs`, `../py`, `../ts`) — outside the contracts project root, which
  Moon cannot list as task outputs. The mechanism is therefore the ordering edge
  plus the committed generated code (an input of each consumer), not output
  caching/hydration of `generate`. Accepted.

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
6. **Determinism & drift (decisions #8/#9)** — `buf.gen.yaml` plugins are
   version-pinned and the prost/tonic crate versions match; the PR drift gate
   (regenerate + `git diff --exit-code`) passes on a clean tree and **fails** on a
   deliberately-stale generated file.

## Out of scope

- Real `paigasus-gateway` *implementation* / consumption of `HealthService` → its
  own issue.
- A *nightly* `codegen-drift.yml` re-affirmation → follow-up. (The PR-level drift
  gate itself is **in scope** — §9 / decision #9.)
- Flipping `paigasus-proto` `publish=false` / ts `private` → **SMA-388** (which
  this unblocks).
- Any further domain schemas beyond the single `HealthService`.

## Follow-ups

- **SMA-388** — flip `paigasus-proto` `publish`/`private` once generated code
  lands (this issue unblocks it).
- Nightly codegen-drift re-affirmation (the PR-level gate lands here; a nightly
  adds defense-in-depth against plugin-registry changes outside a PR).
