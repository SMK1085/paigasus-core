# SMA-398 — Polyglot release-tooling strategy (ADR) + Rust dry-run semver-parity slice

**Status:** Designed (brainstorming complete 2026-06-02)
**Date:** 2026-06-02
**Linear:** [SMA-398](https://linear.app/smaschek/issue/SMA-398/ci-release-tool-dry-run-classification-smoke-job-commit-semver-parity)
**Branch:** `feature/sma-398-ci-release-tool-dry-run-classification-smoke-job-commit`
**Targets:** `main` (currently `f81a3cb`).
**References:** ADR-0005 (one kernel, many bindings — the polyglot coupling that makes versioning a cross-language question); ADR-0010 (lefthook + commitlint; release tools as **downstream** consumers of Conventional Commits); SMA-371 AC-E (the commit→semver parity invariant — origin of this delegation); SMA-361 (CI workflow — shipped the commitlint half of AC-E, deferred this half); SMA-307 (Helikon release-plz conventions: rolling two-job release-PR model, `dependencies_update`, `sort_commits = "newest"`, pre-1.0 semantics, `CARGO_REGISTRY_TOKEN`); SMA-385 (Helikon: `feat(core)` didn't bump — root cause **scope ≠ file-path** + manual-tag metadata loss); SMA-347 / SMA-372 / SMA-382 / SMA-350 (Helikon "0.0.0 trap" escapes); SMA-378 (PyPI package metadata required *before first publish*).
**Spun-out follow-ups (to be created):** **E3** — python-semantic-release dormant config + Py parity adapter; **E4** — semantic-release dormant config + TS parity adapter. Both `relatedTo` SMA-398, `blockedBy` the strategy ADR.

## Context / problem

SMA-371 AC-E requires two CI-side invariants to keep Conventional Commits *honest* end-to-end. SMA-361 shipped the first (commitlint runs in CI with the same pinned config as the local hook). The second — a CI job that runs the **release tooling in dry-run** over synthetic commits and asserts the resulting **semver classification** matches expectation — was deferred here because, as of SMA-361, **`paigasus-core` has no release tooling configured.**

That deferral hides a deeper precondition. An audit during brainstorming found **every artifact in every workspace is a non-publishable `0.0.0` stub**: all four Rust crates are `publish = false`; all four Python packages are `0.0.0` with empty `__init__.py` and a `TODO(SMA-378)` for pre-publish metadata; all seven TS packages are `private: true`. There is nothing to release. So SMA-398 is blocked not just on tooling, but on the *strategy* for introducing it — a strategy that, per CLAUDE.md, is a "significant choice" warranting an **ADR before code**.

The release-plz work referenced by SMA-307/347/372/382/385 all lives in the **older, Rust-only Helikon repo** — a different project. None of it is present here, but its scar tissue is the most valuable input we have: the "0.0.0 trap" and the **scope-vs-file-path** mapping bug (SMA-385) are exactly the failure modes this job must prevent and detect.

## Goal

1. Settle a **polyglot versioning & release strategy** (→ ADR-00XX) that survives three ecosystems (crates.io / PyPI / npm) and avoids re-importing Helikon's traps.
2. Land the **first vertical slice** for Rust: a *dormant* `release-plz` configuration plus a **dry-run semver-parity smoke test** that satisfies SMA-398's AC for the Rust ecosystem.
3. Build that parity test as a **tool-agnostic harness** so the Python/TS adapters (E3/E4) drop into the same expectation-table machinery rather than three bespoke jobs.

After this lands: a PR that changes the release config, the parity harness, or the pinned release-plz version re-runs a CI check that dry-runs release-plz over a fixed table of synthetic Conventional Commits and asserts each maps to the expected semver bump — catching the `feat!:`-vs-`BREAKING CHANGE:`-footer divergence (and the 0.x bump subtleties) that pass commitlint but misclassify in release tooling.

## Strategy decisions (the ADR core)

The decisions below are the substance of **ADR-00XX "Polyglot versioning & release strategy."** They were settled during brainstorming.

| # | Decision | Rationale |
|---|----------|-----------|
| S1 | **Independent per-package versioning.** Each crate / PyPI / npm package versions off its own changed-file history. Kernel→bindings coupling rides on **dependency pins**, not shared version numbers. | Tool-native for release-plz/semantic-release; lowest coordination; keeps the parity job simple. The "same" kernel may sit at different numbers across languages — acceptable, since pins express the real dependency. Rejected: cross-language lockstep (no tool does it natively; reintroduces the bespoke coupling that caused Helikon's bump bugs). |
| S2 | **Per-language tool, all on the release-PR / commit-driven model.** Rust → **release-plz**; Python → **python-semantic-release**; TS → **semantic-release** (+ monorepo path-filtering, exact plugin deferred to E4). | Matches the settled Helikon model (SMA-307) and each ecosystem's idiomatic tool. ADR-0010 already frames release tools as downstream Conventional-Commits consumers. |
| S3 | **`0.1.0` floor; the tool owns *every* tag.** No package starts at `0.0.0`; the first release is `0.1.0`, and from then on the release tool creates **all** tags. Never hand-place a release tag. | Directly encodes the Helikon lessons: the `0.0.0` trap (SMA-347/372/382) and SMA-385's finding that **manually-created tags lack the metadata release-plz uses to track "what's been released,"** silently stopping future bumps. |
| S4 | **Dormant until real.** Config + workflows land but release-PR opening / tag-cutting stays gated off until a package has a real public API; actual registry publish is *additionally* gated on SMA-378 metadata. Near-term, the only thing that runs is the dry-run parity test. | There is nothing to release yet; an active pipeline would cut meaningless stub "releases" and re-expose the very traps S3 guards against. The parity test needs only *config + a baseline + git history* — not publishing — so its value lands now regardless. |
| S5 | **Commit *scope* ≠ release unit; file *path* is.** The ADR states explicitly that release tools map commits to packages by **changed file path**, and that commitlint's workspace scopes (`rs`/`py`/`ts`/`contracts`) do **not** drive per-package bumps. | SMA-385's root cause. Recording it stops contributors (and the parity fixtures) from assuming a `feat(...)` *scope* bumps a specific package. |
| S6 | **Canonical commit→semver contract, pinned to the 0.x regime** (table below), with `always_bump_minor_for_0 = true`. | The contract the parity harness asserts. 0.x behavior is tool- and config-dependent — precisely the polyglot divergence this job exists to surface. |

### S6 detail — the canonical expectation table

release-plz follows **Cargo's SemVer compatibility rules** in `0.x`: a breaking change bumps the **minor** field (`0.1.0 → 0.2.0`), and compatible changes bump the **patch** field. Critically, by **default** `feat:` bumps only *patch* in `0.x` (feat and fix become indistinguishable). The `always_bump_minor_for_0` knob restores the feat/fix distinction. We set it **`true`**, so the contract is:

| Commit | 1.x bump | **0.x bump (our config: `always_bump_minor_for_0 = true`)** | From `0.1.0` |
|--------|----------|-------------------------------------------------------------|--------------|
| `fix: …` | patch | patch | `0.1.1` |
| `feat: …` | minor | minor | `0.2.0` |
| `feat!: …` | major | **minor** | `0.2.0` |
| `…\n\nBREAKING CHANGE: …` (footer) | major | **minor** | `0.2.0` |

Notes:
- **`feat!:` and the `BREAKING CHANGE:` footer must classify identically** — this equivalence is the specific failure mode SMA-371 AC-E names, and the minimum two rows the harness must distinguish from `feat:`.
- In `0.x` both breaking forms land on `0.2.0` (minor), *not* a major bump — the table asserts the **0.x** numbers, not the textbook 1.x ones. A harness that asserted `1.0.0` for `feat!:` would be wrong for this repo's regime.
- The contract is **intent**; each tool's adapter is configured to honor it. When E3/E4 land, any tool whose 0.x defaults disagree is either reconfigured to match or its divergence is documented as a known, tool-specific exception. Surfacing those is the harness's whole point.

## Decision (what this branch delivers)

A **dormant** Rust release configuration and a **tool-agnostic dry-run parity harness** with a release-plz adapter, wired into CI as a per-PR affected check. No active release workflow cuts tags or opens PRs. The strategy itself is captured in ADR-00XX (Notion) and the child issues E3/E4 are created for the other two ecosystems.

Deliverables on this branch:

- `docs:` — ADR-00XX draft / link, and this spec.
- `ci:` / `build:` — pin `release-plz` in `.prototools`.
- `feat(release):` — dormant `rs/release-plz.toml` (S3/S4/S6 settings).
- `feat(ci):` — the parity harness under `ci/release-parity/` (fixture-builder + expectation-table data + release-plz adapter) and its Moon task.
- `feat(ci):` — wire the parity task into `.github/workflows/ci.yml` as an affected per-PR check (or confirm it rides the existing `moon ci` graph — see §9).

## Design

### 1. Independent versioning + coupling via pins (S1)

No workspace-version locking. `rs/` keeps per-crate versions (release-plz's native model). The Rust→Py→TS kernel coupling is *not* a shared number; where a binding depends on the kernel, that's a dependency pin in the consumer's manifest. The parity harness therefore reasons about **one package at a time** and never needs cross-package version math.

### 2. Tool selection (S2)

This slice implements **release-plz** only. python-semantic-release and semantic-release are named in the ADR and deferred to E3/E4. The harness's adapter seam (§7) is the contract those issues implement against.

### 3. `0.1.0` floor, tool-owned tags (S3)

The ADR prescribes: first release of any package sets `0.1.0`; the release tool creates every subsequent tag; humans never hand-write a `*-vX.Y.Z` release commit/tag. The dormant `release-plz.toml` encodes the floor; because the pipeline is dormant (S4), no tags are actually cut on `main` by this work. The **parity fixture** seeds its *own* throwaway baseline tag (`<pkg>-v0.1.0`) inside a temp repo — never touching real repo tags.

### 4. Dormant activation (S4)

"Dormant" is concrete and **verifiable**:
- `rs/release-plz.toml` exists and is valid, but **no workflow triggers release-plz on push to `main`.** Either no `release-plz.yml` is added, or one is added with only a `workflow_dispatch` (manual) trigger and never auto-runs.
- The real crates stay `publish = false`; the config changes nothing about publish state.
- The single observable behavior added is the **dry-run parity check**, which mutates nothing outside its temp dir.
- Verification (§Verification) explicitly asserts no tag/PR/changelog is produced on `main`.

### 5. Scope ≠ release unit (S5)

The harness encodes this lesson structurally: each fixture commit **touches files under the target crate's path** to trigger the bump, and the commit *scope* is treated as cosmetic. A regression note in the harness README records why (SMA-385), so a future maintainer doesn't "simplify" the fixture into scope-only commits that release-plz would ignore.

### 6. The expectation table (S6)

Lives as **data**, not code — e.g. `ci/release-parity/cases.toml` (or `.json`) — so adding a case or porting the table to another tool's adapter is a data edit. Each row: `{ id, commits: [...], touches: [path], expected_version }`, with baseline `0.1.0`. Minimum rows: `fix:`, `feat:`, `feat!:`, `BREAKING CHANGE:` footer (the table in S6 detail).

### 7. The dry-run parity harness (tool-agnostic, reusable)

**Shape:** `fixture-builder + expectation table + per-tool adapter`. The adapter is the only tool-specific piece.

**Adapter contract:** `run(fixture_dir, commits[]) -> proposed_version_string`.
- **release-plz adapter:** in the fixture repo, run `release-plz update --dry-run` (machine-readable output) and parse the proposed version for the target package. semver-check is **disabled in the fixture** so the calculation is purely Conventional-Commit-driven and needs no crates.io network access.
- Future adapters (E3/E4): `semantic-release --dry-run`, `python-semantic-release version --print`.

**Mechanism (Approach A — ephemeral fixture, isolated per case):**
1. Create a temp dir; `git init`; write a minimal Cargo workspace with **one throwaway crate** + a `release-plz.toml` mirroring the real S6 settings.
2. Commit; tag baseline `<pkg>-v0.1.0`.
3. For each table row (one crate per case to avoid `dependencies_update` cascade): create the row's commit(s) touching the crate's files → run the adapter → assert `proposed_version == expected_version`.
4. Aggregate failures; non-zero exit on any mismatch, with a readable diff (`case id: expected X, got Y`).

**Determinism:** release-plz pinned via proto; fixed baseline tag; no network (semver-check off); each case in isolation. No reliance on the host repo's history.

**Self-check (anti-false-green):** one CI-exercised guard case carries a deliberately wrong `expected_version` behind a `--negative-control` mode (or a unit assertion) proving the harness reports red on mismatch. This guards against an adapter that silently returns "no bump" for everything.

**Home:** `ci/release-parity/` — `run.sh` (or a small script in the repo's lingua franca), `cases.toml`, `adapters/release-plz.sh`, `README.md` (records the SMA-385 rationale). Driven by a Moon task.

### 8. The Rust slice (release-plz dormant config + adapter)

- `.prototools`: pin `release-plz` (exact version TBD at implementation — latest stable).
- `rs/release-plz.toml`: `dependencies_update = true`, `sort_commits = "newest"`, `always_bump_minor_for_0 = true`, semver-check posture set (and **off** in the fixture config), per-crate overrides as needed. Mirrors SMA-307 conventions minus the active workflow.
- `ci/release-parity/adapters/release-plz.sh`: the adapter from §7.
- `ci/release-parity/cases.toml`: the S6 table.

### 9. CI wiring (per-PR affected only)

Per the cadence decision, the parity check runs **per-PR on the affected graph only — no nightly.** Two implementation options, pick the one that verifies clean:
- **(a) Moon task on a project** (e.g. a `ci`/`repo`-scoped project) named `release-parity`, with **inputs** = `ci/release-parity/**`, `rs/release-plz.toml`, **and `.prototools`**. Including `.prototools` means a release-plz **version bump re-runs the check at pin-bump time** — preserving tool-drift detection without a nightly. Add `:release-parity` to the `moon ci` target list in `ci.yml`.
- **(b) A dedicated workflow step** invoking `ci/release-parity/run.sh` directly, gated by `paths:` on the same inputs.

Prefer **(a)** — it reuses the existing toolchain setup + affected-graph and keeps branch protection to the single `moon ci` check. The task must have `release-plz` available (proto-managed; `moon setup`/`proto install` already runs in CI per SMA-361).

> **Cadence trade-off (recorded):** per-PR-affected-only means a release-plz upstream change is caught **only when `.prototools` changes** (the pin bump), not continuously. Acceptable: proto pins are explicit, so drift can't arrive silently between pin bumps. Re-add a nightly later if floating-version exposure ever appears.

### 10. Decomposition — ADR + Linear

- **ADR-00XX "Polyglot versioning & release strategy"** (Notion) — the S1–S6 table; written before/with the config code, per CLAUDE.md.
- **SMA-398** (this issue) — re-scoped to *parity harness + release-plz adapter (Rust) + dormant Rust config*. Its AC's "once release tooling exists" precondition is met **for Rust** by the dormant `release-plz.toml` in this same slice. Stays In Progress.
- **E3** — python-semantic-release dormant config + Py parity adapter (uses §7 seam).
- **E4** — semantic-release dormant config + TS parity adapter.
- Relations: E3/E4 `relatedTo` SMA-398, `blockedBy` ADR-00XX.

### 11. Out of scope

- **Actual registry publishing** (crates.io / PyPI / npm) — gated on real APIs + SMA-378 metadata; S4 keeps everything dormant.
- **Active release workflows** (rolling release-PR, tag-cutting, changelog commits) — deferred until a package has a real public API.
- **Python & TS parity adapters** — E3/E4.
- **Nightly drift job** — explicitly dropped per cadence decision (§9).
- **cargo-semver-checks API-breaking detection** — orthogonal to commit-message classification; disabled in the fixture.

## Verification plan (on this branch's PR)

1. **Harness green on the canonical table:** `moon run <proj>:release-parity` (or `ci/release-parity/run.sh`) builds the fixture, dry-runs release-plz over all S6 rows, asserts every `proposed == expected`. Exit 0.
2. **Negative control fails red:** flip one expected value (or run `--negative-control`) and confirm the harness exits non-zero with a readable `expected X, got Y`. Proves no false-green.
3. **`feat!:` ≡ `BREAKING CHANGE:` footer:** confirm both rows independently produce `0.2.0` (the AC's named divergence).
4. **0.x correctness:** confirm `feat:` → `0.2.0` (proves `always_bump_minor_for_0 = true` is active) and `fix:` → `0.1.1`.
5. **Dormancy:** confirm the slice produces **no** tag, release PR, or changelog mutation on `main` — grep the workflow set for any push-triggered release-plz invocation (there must be none), and confirm `release-plz.toml` changes leave `publish = false` intact.
6. **Affected wiring:** a PR touching only an unrelated file does **not** run `release-parity`; a PR touching `rs/release-plz.toml`, `ci/release-parity/**`, or `.prototools` **does** (capture the Moon run summary).

## Acceptance-criteria mapping

| AC (SMA-398) | How satisfied |
|--------------|---------------|
| Once release tooling exists, a CI job dry-runs each configured release tool over fixed synthetic commits (`feat:`, `fix:`, `feat!:`, `BREAKING CHANGE:` footer) and asserts each maps to the expected semver bump. | §6–§9: dormant `release-plz.toml` *is* the configured tooling (for Rust); the harness dry-runs it over the S6 table. Py/TS tools tracked as E3/E4 (the harness is built to absorb them). |
| Catches the SMA-371 failure mode: a commit that passes commitlint but is misclassified — notably `feat!:` vs `BREAKING CHANGE:` footer. | §6 detail + Verification #2/#3: the table distinguishes both breaking forms from `feat:`, and the negative control proves the assertion has teeth. |
| Runs in CI (nightly or per-PR — decide when release tooling lands). | §9: **per-PR on the affected graph**, inputs include `.prototools` so pin bumps re-run it. Nightly explicitly declined. |

## Risks / to-verify during implementation

1. **`release-plz update --dry-run` output contract (§7).** Confirm the exact machine-readable form and the field carrying the proposed version; pin the parse to it. Fallback: `release-plz release-pr --dry-run` if `update` doesn't surface the version cleanly.
2. **`always_bump_minor_for_0` semantics (§6).** Verify empirically that the pinned release-plz version yields `feat:` → `0.2.0` and `feat!:` → `0.2.0` from a `0.1.0` baseline. If a release-plz version changes the knob's behavior, the table is the single source of truth to reconcile.
3. **Offline dry-run (§7).** Confirm `release-plz update --dry-run` with semver-check disabled needs **no** crates.io network in the fixture (CI may be network-restricted). If it still resolves the index, vendor or set a registry override in the fixture.
4. **`dependencies_update` cascade (§7).** Verify one-crate-per-case isolation actually prevents a dependency bump from perturbing the asserted version; if a single-crate fixture still cascades, give each case its own fixture dir.
5. **Moon task tool availability (§9).** Confirm the `release-parity` task sees the proto-pinned `release-plz` on `PATH` after `moon setup` (same mechanism SMA-361 relies on for buf/pnpm/uv).
6. **ADR number.** Allocate the next free ADR number in the Notion ADR index before writing ADR-00XX.
