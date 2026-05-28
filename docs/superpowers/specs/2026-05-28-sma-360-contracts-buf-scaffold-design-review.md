# Review — SMA-360 contracts/buf scaffold design

**Reviews:** [`2026-05-28-sma-360-contracts-buf-scaffold-design.md`](./2026-05-28-sma-360-contracts-buf-scaffold-design.md)
**Reviewer perspective:** staff engineer
**Date:** 2026-05-28
**Sources cross-referenced:** Linear SMA-360 (+ SMA-357, SMA-383 prerequisites), ADR-0004, the canonical [Polyglot Monorepo Scoping §2](https://www.notion.so/368830e8fbaa8101b0ffded7a3de3b53), and the live `paigasus-core` tree.

## Verdict

The scaffold-level design is sound and well-reasoned, and several of its deviations from the acceptance criteria are *corrections* of real defects in the AC and even in the Notion source of truth (see "What it gets right"). It is safe to implement as a scaffold.

However, the spec optimizes for "the empty workspace passes today" and under-specifies the parts that only activate when the first real `.proto` lands or when CI is wired. The `buf.gen.yaml` it commits is a materially reduced copy of the canonical config — it drops every plugin `opt` and `clean: true` with no rationale — and the dependency edges that are the *entire point* of the monorepo (proto → codegen → downstream rebuild) are not established anywhere. Those are the things that will bite, and they will bite silently, weeks later, when someone assumes the committed config is the real one.

The findings below are ordered by how badly they bite and how late they surface.

## What the spec gets right (calibration)

These are genuine strengths and should not be "fixed":

- **buf config relocation (decision #4) is correct.** The AC and the Notion §2 config block both place `buf.yaml` at `contracts/proto/` *and* declare `modules: [{ path: proto }]` — that resolves to `contracts/proto/proto`, which does not exist. The spec moving config to `contracts/` root is the right fix, not a deviation to be reconciled.
- **TS path correction is correct and grounded.** AC and Notion §2 both say `ts/packages/proto/...`; the real package is `ts/packages/paigasus-proto` (confirmed in tree). The spec matches reality.
- **The "Rust task template already references `paigasus-proto-rs` and `contracts:generate`" claim is verified true** — `.moon/templates/rust/moon.yml` lines 7 and 13/17. Decision #1 (scaffold the Rust crate here) is well-justified; SMA-357 created `rs/` with only `paigasus-kernel`, so `paigasus-proto-rs` genuinely doesn't exist yet.
- **The kernel-mirror approach is accurate.** `publish = false` + TODO, `version = "0.0.0"`, SPDX-only `lib.rs` with no module declarations, workspace-inherited fields — all match the real `paigasus-kernel` crate.
- **The SPDX carve-out (§7) is on solid ground.** SMA-383 is *Done*; CONTRIBUTING now exempts config files explicitly. (Minor: the spec lists SMA-383 as merely "related" — it is in fact a *satisfied prerequisite*.)

## Findings summary

| # | Severity | Finding | Bites when |
|---|----------|---------|-----------|
| H1 | High | `buf.gen.yaml` drops all plugin `opt`s present in the canonical config | First real proto generates subtly-wrong code |
| H2 | High | `clean: true` silently dropped — undecided, and conflicts with the `.gitkeep` stubs | First `buf generate`; later, codegen-drift misses deletions |
| H3 | High | No build-order wiring: `contracts:generate` is an orphan task nothing depends on | First proto; the documented affected-graph never materializes |
| M1 | Medium | Canonical source of truth (Notion §2) left stale after local fixes | Next person reading §2 reintroduces the fixed bugs |
| M2 | Medium | `breaking` task can't run (may error) against a `main` with no contracts baseline | When `breaking` is wired into CI / on the introducing PR |
| M3 | Medium | proto-managed `buf` on PATH for Moon `system` tasks is unproven in CI | When `ci.yml` lands; "runs cleanly" verified locally only |
| M4 | Medium | `layer: 'tool'` is outside the repo's documented active-use set | Next config-file PR re-litigates the convention |
| L1 | Low | Linux-aarch64 buf resolution bug is a buried code TODO, not tracked | If/when ARM Linux CI or a dev box appears |
| L2 | Low | `publish = false` TODO has no tracking issue (kernel's cites SMA-376) | Release pipeline silently can't publish `paigasus-proto` |
| L3 | Low | betterproto2 pre-stable risk (ADR-0004) not surfaced or hooked | If betterproto2 stalls and the fallback is needed |

## High-severity

### H1 — `buf.gen.yaml` drops every plugin `opt` from the canonical config

The spec's §3 table lists four plugins and a single option (`target=ts` on the ES plugin). The canonical config in [Notion §2](https://www.notion.so/368830e8fbaa8101b0ffded7a3de3b53) specifies materially more:

```yaml
- remote: buf.build/community/neoeinstein-prost
  opt: [bytes=., file_descriptor_set]
- remote: buf.build/community/neoeinstein-tonic
  opt: [no_include, compile_well_known_types]
- remote: buf.build/bufbuild/es
  opt: [target=ts, import_extension=.js]
```

None of these are cosmetic:

- **`tonic: no_include`** — without it, the tonic plugin emits its own `mod`/include scaffolding that collides with prost's when both write to the *same* `src/generated` directory (which this design intends). This is a hard build break the day services are generated, and it presents as a confusing duplicate-module error far from its cause.
- **`prost: bytes=.`** — generates `Bytes` instead of `Vec<u8>` for byte fields. Changing this *after* code exists is a breaking API change to `paigasus-proto`'s Rust surface; it must be set before the first generation, not retrofitted.
- **`es: import_extension=.js`** — this is the protobuf-es knob for emitting runtime-correct ESM import specifiers. The `@paigasus/proto` package is `"type": "module"`. Today its `tsconfig` uses `moduleResolution: "bundler"` (extensionless imports tolerated), so the omission is *latent* — but its own `package.json` comment states the package will "switch to `./dist/index.js` when tsup wiring lands … in lockstep with flipping `private: false`." The open-core release strategy *requires* that publish step. The moment the package emits real ESM, missing `.js` extensions break consumers under NodeNext resolution.

Because the spec explicitly commits `buf.gen.yaml` now ("output paths configured even though no protos exist yet"), the config it commits *is* the one the next engineer will trust. A reduced config that looks complete is worse than an absent one. **Recommendation:** carry the full opt set from §2 verbatim, or, if any opt is being deliberately deferred, say so inline in the spec with a reason. Silent omission is the failure mode.

### H2 — `clean: true` dropped, undecided, and in tension with the `.gitkeep` stubs

Notion §2's `buf.gen.yaml` opens with `clean: true`. The spec's §3 omits it without comment, which creates an unforced ambiguity for the implementer and a latent correctness gap:

- If the implementer follows the source of truth and sets `clean: true`, then AC verification step 5 (`buf generate` produces no output without erroring) will also **wipe the `generated/` directories**, deleting the very `.gitkeep` stubs that step 7 and §6 rely on. Git doesn't track empty dirs, so the stub dirs vanish. The two ACs become mutually exclusive.
- If the implementer follows the spec and omits it, the scaffold is fine — but once protos exist, **deleting a `.proto` will leave its stale generated output on disk**. The committed-codegen model (ADR-0004) leans on the `codegen-drift.yml` nightly to catch divergence; without `clean`, a regen that *should* remove files won't, and drift detection that only diffs regenerated content can miss orphaned files.

This needs to be an explicit decision, not an omission. **Recommendation:** state the policy directly — e.g. "omit `clean` while the workspace is empty because it conflicts with the `.gitkeep` stubs; add `clean: true` in the same PR that lands the first protos (and removes the stubs)." Tie it to the codegen-drift issue so it isn't lost.

### H3 — `contracts:generate` is an orphan; no consumer declares a dependency on it

Notion §2 calls the proto→downstream affected graph "the single largest day-one win of the monorepo," with the explicit chain: touch a proto → `contracts:generate` runs → `paigasus-proto` builds → downstreams rebuild. That chain requires `paigasus-proto`'s build to *depend on* `contracts:generate`. None of the three proto packages establish that edge:

- `.moon/templates/rust/moon.yml` only emits `deps: ['contracts:generate', '^:build']` for the **`service`** archetype. The **`library`** archetype (which `paigasus-proto-rs` is) deliberately emits no `dependsOn`, to avoid the kernel/proto self-cycle. So the proto crate inherits the plain global `build` (`cargo build`) with **no ordering dependency on codegen**.
- `py/packages/paigasus-proto/moon.yml` and `ts/packages/paigasus-proto/moon.yml` are bare (`$schema/id/layer/language`), with no task dep on `contracts:generate` either.

`paigasus-proto-rs` is special: it is the one library whose *source is the generated code*, so depending on `contracts:generate` is correct and creates no cycle (the dependency points one way: generate → proto build). The spec treats it as a vanilla kernel clone and inherits the cycle-avoidance default that doesn't apply to it. Relying on "generate writes into `src/`, which is a build input, so caching busts" is not equivalent to an ordered dependency — Moon may schedule `contracts:generate` and `paigasus-proto-rs:build` without guaranteeing generate-before-build.

This is reasonable to *defer* (there are no protos to order yet), but right now `contracts:generate` is a task nothing references, and wiring it later is easy to forget precisely because everything is green without it. **Recommendation:** either (a) have `paigasus-proto-rs:moon.yml` override the inherited `build`/`test` with `deps: ['contracts:generate']` now (and note the py/ts equivalents as explicit follow-ups), or (b) add a sentence to "Out of scope" naming the un-wired affected-graph edges and the issue that will close them. Don't leave it implicit.

## Medium-severity

### M1 — The canonical source of truth is left stale after the spec fixes it locally

The spec correctly fixes three defects (buf.yaml location, TS path) that originate in Notion §2 itself, and reduces the config (H1/H2). The result is that **Notion §2 — the document ADR-0004 and SMA-360 both point to as authoritative — is now wrong** on: the `buf.yaml`/`path: proto` inconsistency, the `ts/packages/proto` path, and (depending on H1/H2 resolution) the opt set. The next issue that reads §2 verbatim (the first real protos, or `paigasus-workflow`) will reintroduce exactly the bugs this spec fixed. A local spec correcting a global doc, without a task to update the global doc, guarantees re-litigation. **Recommendation:** add a follow-up to reconcile Notion §2 with the as-built config, and reference it from the spec.

### M2 — `breaking` against `main` has no baseline at bootstrap and may error

The `breaking` task is `buf breaking --against '.git#branch=main,subdir=contracts'`. On the PR that introduces `contracts/`, `main` has only `contracts/README.md` — no `buf.yaml`, no module. Depending on buf's behavior for a missing module at the against-ref, the task either no-ops or errors ("no module / could not find buf.yaml"). The spec asserts "All four run cleanly on the empty workspace," which is true *locally* but optimistic for `breaking` once it runs in CI against a baseline-less `main`. The AC only verifies `lint`, so this won't be caught by the listed verification. **Recommendation:** note that `breaking` is effectively a no-op until `main` carries a contracts baseline, and confirm buf's missing-baseline behavior before wiring `breaking` into `moon ci`.

### M3 — proto-installed `buf` on PATH for Moon `system` tasks is unproven in CI

`buf` is pinned as a **proto plugin**, not a Moon toolchain. CONTRIBLUTING's toolchain note covers only Rust/Node/Python via `.moon/toolchain.yml`; Moon will not inject `buf` onto a task's PATH the way it does for managed toolchains. The `contracts` project has no `language`, so its tasks run under the **system** toolchain and rely on `buf` already being on the shell PATH (i.e. proto's shim dir). Locally that holds after `proto install`. In CI it requires the workflow to activate proto's shims *before* `moon ci` — and `.github/workflows/` currently contains only a `.gitkeep`, so this has never run in CI. AC "moon run contracts:lint runs cleanly" is therefore validated on dev machines only. **Recommendation:** add a verification that `moon run contracts:lint` finds `buf` in a clean, shell-rc-free environment, and capture the proto-activation requirement as an input to the future `ci.yml`.

### M4 — `layer: 'tool'` is outside the repo's documented active-use set

CONTRIBUTING (just codified in SMA-383) enumerates the `layer:` values "in active use" as `library`, `application`, and `configuration`, and says "pick `library` if unsure." The spec's §4 uses `layer: 'tool'` for `contracts`. `tool` is a valid Moon value and is arguably the *best semantic fit* for a codegen project — but it silently introduces a fourth value the convention doc doesn't sanction, days after that convention was deliberately written down. This is the classic "defensible choice that diverges from a fresh team convention" trap. **Recommendation:** either conform to a sanctioned value (`configuration` is the closest precedent — `py/moon.yml` uses it for a non-language aggregator) or, if `tool` is intended, extend CONTRIBUTING's list and SMA-383's convention in the same PR so it's a decision, not a drift.

## Low-severity / hygiene

- **L1 — Linux-aarch64 buf asset bug.** The known `aarch64 → arm64` mis-resolution is recorded only as a TODO in the vendored `buf.toml` and "out of scope." ARM Linux runners and dev boxes are common enough by 2026 that a buried code comment is the wrong tracking mechanism — a contributor on Linux-arm64 hits a silent wrong-asset download with no Linear trail. Convert it to a tracked issue.
- **L2 — `publish = false` TODO is untracked.** The kernel cites `TODO(SMA-376)`; the spec's proto-crate TODO is a bare "flip once generated code lands." Since `paigasus-proto` is crates.io-bound in the open-core release strategy, an un-flipped `publish = false` silently excludes it from the release pipeline. Mirror the kernel and cite a tracking issue.
- **L3 — betterproto2 maturity risk not surfaced.** ADR-0004 flags betterproto2 as pre-stable (0.x) with a documented fallback (grpcio-tools + mypy-protobuf). The spec commits the betterproto plugin with no note of this risk or hook to the fallback. A one-line pointer keeps the decision discoverable when the Python output is first generated.
- **Nit — googleapis dep with no importer.** `deps: [buf.build/googleapis/googleapis]` and a committed `buf.lock` are declared before any `.proto` imports googleapis. Harmless, but it's a pinned external dependency the workspace doesn't yet use; worth a comment so it isn't mistaken for a live import.
- **Nit — py `generated/` package init.** The Python `generated/` stub holds only `.gitkeep`; it won't be an importable subpackage until betterproto emits an `__init__.py`. Fine for the scaffold, but worth confirming the generator emits package markers rather than assuming namespace-package behavior.

## Suggested additions to the acceptance criteria / follow-ups

1. Commit the **full** `buf.gen.yaml` opt set, or document each deferral inline (H1).
2. Make the `clean:` decision explicit and tie its re-introduction to the first-protos / codegen-drift issue (H2).
3. Name the un-wired `contracts:generate` consumer edges in "Out of scope" with a tracking issue, or wire `paigasus-proto-rs:build → contracts:generate` now (H3).
4. Open a follow-up to reconcile Notion §2 with the as-built config (M1).
5. File tracked issues for: Linux-aarch64 buf resolution (L1) and the `paigasus-proto` `publish` flip (L2).
6. Resolve `layer: 'tool'` vs. the documented set before merge (M4).

## Sources

- Spec under review: `docs/superpowers/specs/2026-05-28-sma-360-contracts-buf-scaffold-design.md`
- [Linear SMA-360 — Bootstrap contracts/ proto workspace with buf scaffold](https://linear.app/smaschek/issue/SMA-360/bootstrap-contracts-proto-workspace-with-buf-scaffold) (AC, relations; blocked by SMA-355/356, blocks SMA-363, related SMA-383)
- [Linear SMA-357 — Bootstrap rs/ Cargo workspace](https://linear.app/smaschek/issue/SMA-357/bootstrap-rs-cargo-workspace-with-libsbindingsservices-layout) (Done; created `rs/` with kernel only)
- [Linear SMA-383 — moon.yml field-order + config SPDX carve-out](https://linear.app/smaschek/issue/SMA-383/document-moonyml-field-order-config-file-spdx-carve-out-in) (Done; layer values + SPDX exemption)
- [Notion — ADR-0004: Protobuf + buf as the single source of truth](https://www.notion.so/368830e8fbaa81a99777ceb7421b64d7)
- [Notion — Polyglot Monorepo Scoping §2](https://www.notion.so/368830e8fbaa8101b0ffded7a3de3b53) (canonical `buf.yaml` / `buf.gen.yaml`, affected-graph contract)
- Repo: `.moon/templates/rust/moon.yml`, `.moon/tasks/rust.yml`, `rs/Cargo.toml`, `rs/crates/libs/paigasus-kernel/{Cargo.toml,moon.yml,src/lib.rs}`, `py|ts/packages/paigasus-proto/`, `CONTRIBUTING.md`, `.github/workflows/`
