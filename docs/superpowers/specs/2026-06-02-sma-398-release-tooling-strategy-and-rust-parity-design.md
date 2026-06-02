# SMA-398 — Polyglot release-tooling strategy (ADR) + Rust dry-run semver-parity slice

**Status:** Designed (brainstorming complete 2026-06-02; **staff-eng review incorporated 2026-06-02 — F1–F5**)
**Date:** 2026-06-02
**Linear:** [SMA-398](https://linear.app/smaschek/issue/SMA-398/ci-release-tool-dry-run-classification-smoke-job-commit-semver-parity)
**Branch:** `feature/sma-398-ci-release-tool-dry-run-classification-smoke-job-commit`
**Targets:** `main` (currently `f81a3cb`).
**References:** ADR-0005 (one kernel, many bindings); **Polyglot Monorepo Scoping doc §3 #4 + §4** (the lockstep mandate — "version binding artifacts together… don't allow them to drift"; single-tag synchronized publish); ADR-0010 (lefthook + commitlint; release tools as **downstream** Conventional-Commits consumers); SMA-371 AC-E (the parity invariant — origin of this delegation); SMA-361 (CI workflow — shipped the commitlint half of AC-E, deferred this half); SMA-307 (Helikon release-plz conventions); SMA-385 (Helikon: `feat(core)` didn't bump — root cause **scope ≠ file-path** + manual-tag metadata loss); SMA-347 / SMA-372 / SMA-382 / SMA-350 (Helikon "0.0.0 trap" escapes); SMA-378 (PyPI package metadata required *before first publish*).
**Spun-out follow-ups (to be created):** **E3** — python-semantic-release dormant config + Py parity adapter (governs `paigasus-ml`, `paigasus-workflows` only); **E4** — semantic-release dormant config + TS parity adapter (governs `@paigasus/sdk`, `@paigasus/ui` only); **E-activate** — first activation (`0.0.0 → 0.1.0`, lockstep wiring, live workflows). All `relatedTo` SMA-398; E3/E4/E-activate `blockedBy` the strategy ADR.

## Review changes (2026-06-02)

A staff-eng review raised five findings; all five were verified and incorporated:

- **F1 (High):** the original S1 ("independent per-package versioning") *reversed* the scoping doc's lockstep mandate. Resolved → **hybrid**: lockstep within the kernel/proto families, independent across unrelated packages (S1 below). The ADR **refines** the scoping doc rather than silently diverging.
- **F2 (Med):** in 0.x with `always_bump_minor_for_0 = true`, `feat!:` and `feat:`+footer are *degenerate* (both = `feat:` = `0.2.0`). The discriminating cases now use a **patch-base** (`fix!:`, `fix:`+footer); 1.x columns staged but unasserted (S6, §6).
- **F3 (Med):** the fixture `release-plz.toml` is now **derived from the real `rs/release-plz.toml`**, not hand-mirrored (§7).
- **F4 (Med):** the original single-crate fixture couldn't test the path→package **attribution** that was SMA-385's actual bug; the fixture is now **multi-crate** with an attribution case (§5, §7).
- **F5 (Low):** the `0.0.0 → 0.1.0` first-activation step is named as the riskiest activation moment and routed to **E-activate** (§11).

## Context / problem

SMA-371 AC-E requires two CI-side invariants to keep Conventional Commits *honest* end-to-end. SMA-361 shipped the first (commitlint in CI with the same pinned config as the local hook). The second — a CI job that runs the **release tooling in dry-run** over synthetic commits and asserts the resulting **semver classification** — was deferred here because, as of SMA-361, **`paigasus-core` has no release tooling configured.**

That deferral hides a deeper precondition. An audit found **every artifact in every workspace is a non-publishable `0.0.0` stub**: all four Rust crates `publish = false`; all four Python packages `0.0.0` with empty `__init__.py` and a `TODO(SMA-378)`; all seven TS packages `private: true`. There is nothing to release — so SMA-398 is blocked on the *strategy* for introducing tooling, which (per CLAUDE.md) is a "significant choice" warranting an **ADR before code**.

The release-plz work in SMA-307/347/372/382/385 lives in the **older, Rust-only Helikon repo**. Its scar tissue is the most valuable input we have: the "0.0.0 trap" and the **scope-vs-file-path** mapping bug (SMA-385) are exactly the failure modes this job must prevent and detect.

## Goal

1. Settle a **polyglot versioning & release strategy** (→ ADR-00XX) that honors the scoping doc's lockstep mandate for the kernel/proto artifacts while staying tool-native, and avoids re-importing Helikon's traps.
2. Land the **first vertical slice** for Rust: a *dormant* `release-plz` configuration plus a **dry-run semver-parity smoke test** that satisfies SMA-398's AC for the Rust ecosystem.
3. Build that parity test as a **tool-agnostic, multi-crate harness** so the Python/TS adapters (E3/E4) drop into the same expectation-table machinery.

## Strategy decisions (the ADR core)

The decisions below are the substance of **ADR-00XX "Polyglot versioning & release strategy."**

| # | Decision | Rationale |
|---|----------|-----------|
| **S1** | **Hybrid versioning. Lockstep *within* the kernel and proto families; independent *across* unrelated packages.** The **kernel family** (`paigasus-kernel` crate + the PyO3/napi/wasm binding crates + the `paigasus-kernel` Py wrapper + `@paigasus/kernel`) and the **proto family** (`paigasus-proto` crate + generated Py/TS) each carry **one version, driven by the Rust crate** — the wheel/npm artifacts are **maturin/napi byproducts** of the Rust publish, so lockstep is a *build-time derivation*, not cross-tool coordination. Genuinely independent packages (`paigasus-ml`, `paigasus-workflows`, `@paigasus/sdk`, `@paigasus/ui`) version per-package on their own changed-file history. Apps (`console`, `docs`) are `private`, never published. | Honors the **Polyglot Monorepo Scoping doc §3 #4** ("version binding artifacts together… don't allow them to drift") and **§4** (single-tag synchronized publish) for the artifacts those passages actually describe, while keeping per-package independence where no cross-language twin exists. The ADR **refines** the scoping doc — scoping its lockstep mandate explicitly to the kernel/proto families. Rejected: *blanket* independence (drops the kernel-drift guarantee — "which `@paigasus/kernel` matches crate `0.3.0`?" becomes unanswerable) and *full* single-repo version (forces `ml`/`ui` to share a meaningless number). |
| **S2** | **Per-language tool for the independent packages; lockstep families publish as Rust byproducts.** Rust crates → **release-plz**. `paigasus-ml` / `paigasus-workflows` (Py-native) → **python-semantic-release**. `@paigasus/sdk` / `@paigasus/ui` (TS-native) → **semantic-release**. The kernel/proto Py & TS packages are **not** governed by the Py/TS release tools — their versions derive from the Rust crate at maturin/napi publish time. | Each ecosystem's idiomatic tool, on the release-PR / commit-driven model (matches Helikon SMA-307). Stops E3/E4 from trying to version the binding wrappers (which have no independent version to compute). |
| **S3** | **`0.1.0` floor; the tool owns *every* tag.** No package starts at `0.0.0`; first release is `0.1.0`; thereafter the release tool creates **all** tags. Never hand-place a release tag. | Encodes the Helikon lessons: the `0.0.0` trap (SMA-347/372/382) and SMA-385's finding that **manually-created tags lack the metadata release-plz uses to track "what's been released,"** silently stopping future bumps. |
| **S4** | **Dormant until real.** Config + workflows land but release-PR opening / tag-cutting stays gated off until a package has a real public API; registry publish is *additionally* gated on SMA-378 metadata. Near-term, the only thing that runs is the dry-run parity test. | Nothing is releasable yet; an active pipeline would cut meaningless stub releases and re-expose the very traps S3 guards. The parity test needs only *config + a baseline + git history* — not publishing — so its value lands now. |
| **S5** | **Commit *scope* ≠ release unit; file *path* is.** The ADR states explicitly that release tools map commits to packages by **changed file path**; commitlint's workspace scopes (`rs`/`py`/`ts`/`contracts`) do **not** drive per-package bumps. | SMA-385's root cause. Recording it stops contributors (and fixtures) from assuming a `feat(...)` *scope* bumps a specific package. **Tested**, not just documented — see the attribution case (§7, F4). |
| **S6** | **Canonical commit→semver contract, pinned to the 0.x regime** (table below), with `always_bump_minor_for_0 = true`. The breaking-vs-non-breaking distinction is exercised with a **patch-base** (`fix`), because on a `feat` base it is degenerate in 0.x. | The contract the harness asserts. See the S6 detail for why the patch-base matters. |

### S1 mechanism (deferred to E-activate)

How the lockstep number propagates — the maturin/napi version source, and whether the binding crates share a number via release-plz's shared-version feature or derive it at build time — is an **activation-time** concern. This dormant slice records the *intent* (one version for the kernel family, driven by the Rust crate); the wiring is designed in **E-activate**, not here. The parity harness (below) is unaffected, since it tests per-crate *classification*, which is identical under lockstep or independence.

### S6 detail — the canonical expectation table

release-plz follows **Cargo's SemVer rules** in `0.x`: a breaking change bumps the **minor** field (`0.1.0 → 0.2.0`); compatible changes bump **patch**. By **default** `feat:` bumps only patch in `0.x` (feat and fix collapse); `always_bump_minor_for_0 = true` restores the feat/fix distinction. We set it **`true`**.

**The 0.x degeneracy (F2).** With the knob on, `feat:` → minor, so adding a breaking marker to a `feat` (`feat!:`, or `feat:`+`BREAKING CHANGE:` footer) **changes nothing** — all three land on `0.2.0`. Asserting those rows tests nothing. The breaking marker only changes the bump on a **patch-base** type (`fix`), where the non-breaking form is patch and the breaking form is minor. So the *discriminating* cases use a `fix` base:

| Case | Commit | 0.x bump (`always_bump_minor_for_0=true`) | From `0.1.0` | Role |
|------|--------|-------------------------------------------|--------------|------|
| `fix` | `fix: …` | patch | `0.1.1` | non-breaking patch baseline |
| `feat` | `feat: …` | minor | `0.2.0` | proves feat ≠ fix (knob active) |
| `fix-bang` | `fix!: …` | minor | `0.2.0` | **breaking marker on patch-base → caught** |
| `fix-footer` | `fix: …` + `BREAKING CHANGE:` footer | minor | `0.2.0` | **breaking footer on patch-base → caught; ≡ `fix-bang`** |
| `feat-bang` *(staged, asserted)* | `feat!: …` | minor | `0.2.0` | degenerate in 0.x (= `feat`); asserted only to lock the current number, flagged as non-discriminating |

What the job actually verifies **in 0.x**: **under-classification** — a tool that drops the `!` or ignores the `BREAKING CHANGE:` footer yields `0.1.1` (patch) where the table expects `0.2.0` (minor) → red. The AC's named equivalence ("`feat!:` vs `BREAKING CHANGE:` footer must classify the same") is exercised as `fix-bang ≡ fix-footer` (both `0.2.0`, both ≠ plain `fix`). The genuine *breaking-vs-feature magnitude* separation (breaking → **major**) only becomes testable at **1.0** — so `cases.toml` carries a **1.x expectation column, documented but unasserted**, ready for the table regeneration the 1.0 transition forces:

| Case | 1.x bump | 1.x version (from `1.0.0`) |
|------|----------|----------------------------|
| `fix` | patch | `1.0.1` |
| `feat` | minor | `1.1.0` |
| `fix-bang` / `fix-footer` | major | `2.0.0` |
| `feat-bang` | major | `2.0.0` |

The contract is **intent**; each tool's adapter is configured to honor it. When E3/E4 land, any tool whose 0.x defaults disagree is reconfigured to match or its divergence is documented as a known exception — surfacing those is the harness's point.

## Decision (what this branch delivers)

A **dormant** Rust release configuration and a **tool-agnostic, multi-crate dry-run parity harness** with a release-plz adapter, wired into CI as a per-PR affected check. No active release workflow cuts tags or opens PRs. The strategy is captured in ADR-00XX (Notion); child issues E3/E4/E-activate are created.

Deliverables on this branch:

- `docs:` — ADR-00XX draft/link, this spec.
- `build:` — pin `release-plz` in `.prototools`.
- `feat(release):` — dormant `rs/release-plz.toml` (S3/S4/S6 settings).
- `feat(ci):` — the parity harness under `ci/release-parity/` (multi-crate fixture-builder + expectation-table data + release-plz adapter) and its Moon task.
- `feat(ci):` — wire the parity task into CI as an affected per-PR check.

## Design

### 1. Hybrid versioning + coupling (S1)

`rs/` keeps per-crate versions in release-plz's native model. The **kernel and proto families lockstep** (one version driven by the Rust crate; Py/TS artifacts are maturin/napi byproducts). The **independent packages** (`paigasus-ml`, `paigasus-workflows`, `@paigasus/sdk`, `@paigasus/ui`) version per-package. Cross-package coupling otherwise rides on dependency pins. The parity harness reasons about classification **one crate at a time** and never needs cross-package version math; the lockstep *propagation* is E-activate's concern.

### 2. Tool selection (S2)

This slice implements **release-plz** only. python-semantic-release (for `paigasus-ml`/`paigasus-workflows`) and semantic-release (for `@paigasus/sdk`/`@paigasus/ui`) are named in the ADR and deferred to E3/E4. The kernel/proto wrappers are explicitly **out** of those tools' scope (byproducts of the Rust release). The harness's adapter seam (§7) is the contract E3/E4 implement against.

### 3. `0.1.0` floor, tool-owned tags (S3)

The ADR prescribes: first release sets `0.1.0`; the tool creates every subsequent tag; humans never hand-write a release tag/commit. The dormant `release-plz.toml` encodes the floor; because the pipeline is dormant (S4), **no tags are cut on `main`** by this work. The fixture seeds its *own* throwaway baseline tags inside a temp repo, never touching real repo tags.

### 4. Dormant activation (S4)

"Dormant" is concrete and **verifiable**:
- `rs/release-plz.toml` exists and is valid, but **no workflow triggers release-plz on push to `main`** (either no `release-plz.yml`, or one with only a manual `workflow_dispatch` trigger). **Lean: omit the workflow entirely**; E-activate adds it.
- Real crates stay `publish = false`.
- The only observable behavior added is the dry-run parity check, which mutates nothing outside its temp dir.

### 5. Scope ≠ release unit, *tested* (S5, F4)

The original spec claimed the harness "guards SMA-385"; that was overstated for a single-crate fixture (which can only test bump *type*, not *attribution*). SMA-385 was a **path→package mapping** failure in a *multi-crate* workspace. The harness now uses a **multi-crate fixture** (≥2 independent crates, no dep edge) and asserts attribution directly: a commit touching crate **A**'s files bumps **A** and leaves **B** at baseline. Fixture commits touch files under the target crate's path; the commit *scope* is treated as cosmetic, with a `README` note recording the SMA-385 rationale so a maintainer doesn't "simplify" the fixture into scope-only commits release-plz would ignore.

### 6. The expectation table (S6, F2)

Lives as **data** (`ci/release-parity/cases.toml`), each row `{ id, commits: [...], touches: [crate], expected_0x, expected_1x (unasserted), discriminating: bool }`, baseline `0.1.0`. The asserted rows are `fix`, `feat`, `fix-bang`, `fix-footer`, `feat-bang` (per S6 detail) plus the `attribution` case (§5). The 1.x column is carried but not asserted until the 1.0 transition.

### 7. The dry-run parity harness (tool-agnostic, multi-crate, reusable)

**Shape:** `fixture-builder + expectation table + per-tool adapter`. The adapter is the only tool-specific piece.

**Adapter contract:** `bump(fixture_dir, target_crate) -> resulting_version`.
- **release-plz adapter:** run release-plz's version-update against the **disposable fixture** and read the resulting version from the crate manifest. Because the fixture is throwaway, letting `release-plz update` write into it and reading the bumped `version` is equivalent to a dry-run against the real repo — and **avoids fragile log/JSON parsing** of `--dry-run` output (resolves the original Risk #1). semver-check is **off** in the fixture, so the calculation is purely Conventional-Commit-driven and needs no crates.io network.
- Future adapters (E3/E4): `python-semantic-release version --print`, `semantic-release --dry-run`.

**Fixture config derived from the real config (F3).** The fixture's `release-plz.toml` is **not** a hand-maintained mirror. The harness reads `rs/release-plz.toml`, copies the **classification-relevant `[workspace]` keys** (notably `always_bump_minor_for_0`), forces the semver-check **off**, and **omits the per-package `[[package]]` overrides** (they reference real crates and don't affect classification). This guarantees the harness exercises **production classification settings**; a change to those settings in `rs/release-plz.toml` flows into the fixture automatically (and re-runs the check via §9 inputs).

**Mechanism (Approach A — ephemeral multi-crate fixture):**
1. Temp dir; `git init`; write a minimal Cargo workspace with **two independent crates** (`fixture_a`, `fixture_b`, no dep edge) + the derived `release-plz.toml`.
2. Commit; tag baselines `fixture_a-v0.1.0`, `fixture_b-v0.1.0`.
3. For each table row: create the row's commit(s) **touching the target crate's files** → run the adapter → assert `resulting_version == expected_0x` for the target, **and** (attribution rows) the untouched crate stays at baseline.
4. Aggregate; non-zero exit on any mismatch with a readable `case id: expected X, got Y`.

**Determinism:** release-plz pinned via proto; fixed baseline tags; independent crates so `dependencies_update` can't cascade between them; offline (semver-check off). No reliance on host-repo history.

**Self-check (anti-false-green):** a guard run with a deliberately wrong expected value (a `--negative-control` mode) proves the harness reports **red** on mismatch — guarding against an adapter that silently returns "no bump."

**Home:** `ci/release-parity/` — `run.sh` (or a small script), `cases.toml`, `adapters/release-plz.sh`, `README.md` (records the SMA-385 rationale + the derived-config invariant). Driven by a Moon task.

### 8. The Rust slice (release-plz dormant config + adapter)

- `.prototools`: pin `release-plz` (exact version at implementation — latest stable).
- `rs/release-plz.toml`: `dependencies_update = true`, `sort_commits = "newest"`, `always_bump_minor_for_0 = true`, semver-check posture set, per-crate overrides as needed. Mirrors SMA-307 conventions **minus the active workflow**.
- `ci/release-parity/adapters/release-plz.sh`: the adapter (§7).
- `ci/release-parity/cases.toml`: the S6 table.

### 9. CI wiring (per-PR affected only)

The parity check runs **per-PR on the affected graph only — no nightly** (cadence decision). Preferred mechanism: a Moon task `release-parity` on a `ci`/`repo`-scoped project, with **inputs** = `ci/release-parity/**`, `rs/release-plz.toml`, **and `.prototools`** (so a release-plz **pin bump re-runs the check** — preserving tool-drift detection without a nightly). Add `:release-parity` to the `moon ci` target list. The task needs proto-pinned `release-plz` on `PATH` after `moon setup` (same mechanism SMA-361 uses for buf/pnpm/uv).

> **Cadence trade-off (recorded):** per-PR-affected-only means an upstream release-plz change is caught **only when `.prototools` changes** (the pin bump), not continuously. Acceptable: proto pins are explicit, so drift can't arrive silently. Re-add a nightly later if floating-version exposure ever appears.

### 10. Decomposition — ADR + Linear

- **ADR-00XX "Polyglot versioning & release strategy"** (Notion) — the S1–S6 table; written before/with the config code. **Refines** the scoping doc §3 #4 / §4 (scopes the lockstep mandate to the kernel/proto families); the Notion scoping doc gets a back-reference note.
- **SMA-398** (this issue) — re-scoped to *parity harness + release-plz adapter (Rust) + dormant Rust config*. Its AC's "once release tooling exists" precondition is met **for Rust** by the dormant `release-plz.toml`. Stays In Progress.
- **E3** — python-semantic-release dormant config + Py parity adapter (`paigasus-ml`, `paigasus-workflows`).
- **E4** — semantic-release dormant config + TS parity adapter (`@paigasus/sdk`, `@paigasus/ui`).
- **E-activate** — first activation: `0.0.0 → 0.1.0`, kernel/proto lockstep wiring, live workflows (see §11/F5).
- Relations: E3/E4/E-activate `relatedTo` SMA-398, `blockedBy` ADR-00XX.

### 11. Out of scope

- **Actual registry publishing** — gated on real APIs + SMA-378 metadata; S4 keeps everything dormant.
- **Active release workflows** (rolling release-PR, tag-cutting, changelog commits) — deferred to E-activate.
- **First-activation `0.0.0 → 0.1.0` (F5).** Every package is `0.0.0` today; activation must move each to `0.1.0` and let release-plz cut the **first** tag *without a human hand-placing it* — the exact metadata-loss trap S3/SMA-385 warns about. **This is the single riskiest activation step**; it is named here and routed to **E-activate**, not improvised later.
- **Kernel/proto lockstep propagation mechanism** — maturin/napi version source, release-plz shared-version wiring → E-activate (S1 mechanism note).
- **Python & TS parity adapters** — E3/E4.
- **Nightly drift job** — explicitly declined (§9).
- **cargo-semver-checks API-breaking detection** — orthogonal to commit-message classification; off in the fixture.

## Verification plan (on this branch's PR)

1. **Harness green on the canonical table:** `moon run <proj>:release-parity` builds the multi-crate fixture, runs release-plz over all S6 rows, asserts every `resulting == expected_0x`. Exit 0.
2. **Negative control fails red:** flip one expected value (or `--negative-control`); confirm non-zero exit with `expected X, got Y`. Proves no false-green.
3. **Discriminating breaking cases (the AC target, 0.x):** confirm `fix-bang` → `0.2.0` and `fix-footer` → `0.2.0` (equal), each ≠ plain `fix` → `0.1.1`. Confirm a fixture variant that *drops* the `!`/footer yields `0.1.1` (the caught under-classification).
4. **Knob active:** confirm `feat` → `0.2.0` (proves `always_bump_minor_for_0 = true`).
5. **Attribution (F4):** confirm a commit touching `fixture_a` bumps `fixture_a` and leaves `fixture_b` at `0.1.0`.
6. **Derived config (F3):** confirm the fixture `release-plz.toml` is generated from `rs/release-plz.toml` (e.g. flipping `always_bump_minor_for_0` in the real file changes the `feat` row's result), not from a hardcoded copy.
7. **Dormancy:** confirm the slice produces **no** tag/PR/changelog on `main` — grep the workflow set for any push-triggered release-plz invocation (none), and confirm `publish = false` intact.
8. **Affected wiring:** a PR touching only an unrelated file does **not** run `release-parity`; a PR touching `rs/release-plz.toml`, `ci/release-parity/**`, or `.prototools` **does**.

## Acceptance-criteria mapping

| AC (SMA-398) | How satisfied |
|--------------|---------------|
| Once release tooling exists, a CI job dry-runs each configured release tool over fixed synthetic commits and asserts each maps to the expected semver bump. | §6–§9: dormant `release-plz.toml` *is* the configured tooling (Rust); the harness runs it over the S6 table against a disposable fixture. Py/TS tracked as E3/E4. |
| Catches the SMA-371 failure mode: a commit that passes commitlint but is misclassified — notably `feat!:` vs `BREAKING CHANGE:` footer. | §6 detail + Verification #2/#3: in 0.x the job catches **under-classification** (dropped `!`/footer → patch) via the **patch-base** `fix-bang`/`fix-footer` cases (the `feat`-base form is degenerate in 0.x — documented, with 1.x cases staged for when breaking→major makes it discriminating). The negative control proves the assertion has teeth. |
| Runs in CI (nightly or per-PR — decide when release tooling lands). | §9: **per-PR on the affected graph**, inputs include `.prototools` so pin bumps re-run it. Nightly declined. |

## Risks / to-verify during implementation

1. **release-plz version-update mechanics (§7).** Confirm `release-plz update` writing into the disposable fixture yields a readable bumped `version` in the crate manifest (preferred over `--dry-run` log parsing). If `update` refuses without a remote/registry, fall back to `release-plz update --dry-run` + parse.
2. **`always_bump_minor_for_0` semantics (§6).** Verify empirically that the pinned release-plz yields `feat:` → `0.2.0`, `fix:` → `0.1.1`, `fix!:`/`fix:`+footer → `0.2.0` from a `0.1.0` baseline.
3. **Offline (§7).** Confirm semver-check-off update needs **no** crates.io network in the fixture; if it still resolves the index, set a registry override / vendor in the fixture.
4. **`dependencies_update` isolation (§7).** Confirm two independent (no-dep-edge) fixture crates prevent any cross-crate cascade so the attribution assertion holds; if not, give each case its own fixture dir.
5. **Derived-config extraction (§7, F3).** Confirm copying only `[workspace]` classification keys (and forcing semver-check off, dropping `[[package]]`) produces a valid fixture config that still reflects production classification.
6. **Moon task tool availability (§9).** Confirm `release-parity` sees proto-pinned `release-plz` on `PATH` after `moon setup`.
7. **ADR number.** Allocate the next free ADR number in the Notion ADR index before writing ADR-00XX.
