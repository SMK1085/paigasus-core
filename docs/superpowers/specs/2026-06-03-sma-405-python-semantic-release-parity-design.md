# SMA-405 — python-semantic-release dormant config + Python semver-parity adapter

**Status:** Designed (brainstorming complete 2026-06-03)
**Date:** 2026-06-03
**Linear:** [SMA-405](https://linear.app/smaschek/issue/SMA-405/ci-python-semantic-release-dormant-config-py-semver-parity-adapter)
**Branch:** `feature/sma-405-ci-python-semantic-release-dormant-config-py-semver-parity`
**Targets:** `main` (currently `98bee2d`).
**References:** SMA-398 (parent — strategy ADR + the tool-agnostic Rust parity slice this builds on; spec `docs/superpowers/specs/2026-06-02-sma-398-release-tooling-strategy-and-rust-parity-design.md`); **ADR-0011** (Polyglot versioning & release strategy, Notion) — specifically **S1/S2** (the kernel/proto Py packages are **derived** artifacts whose versions track the Rust/contract source — `paigasus-kernel` Py is a **maturin** byproduct of the PyO3 binding, `paigasus-proto` Py is a **buf codegen** byproduct of `contracts/` (ADR-0004), *not* a maturin build — so both are out of PSR scope; `paigasus-ml`/`paigasus-workflows` are independent Py-native packages → in scope), **S4** (dormant: config + parity check only, no live workflow), **S6** (canonical commit→semver contract pinned to 0.x; "any tool whose 0.x defaults disagree is reconfigured to match or its divergence is documented as a known exception"). This issue is the spun-out **E3** from the SMA-398 spec.

> **ADR-0011 wording note (review F4):** ADR-0011 S1/S2 currently frames *both* kernel and proto Py packages as "maturin byproducts." That is precise for the kernel but not for proto (codegen, not maturin). The conclusion is unaffected for SMA-405, but the **E-activate** wiring differs (proto's version propagates from the codegen pipeline, not a maturin build), so the ADR text should be corrected before E-activate.

## Context / problem

SMA-398 landed a **tool-agnostic, multi-crate dry-run semver-parity harness** under `ci/release-parity/`:

- `run.sh` — the ecosystem-agnostic core. It builds a disposable fixture, applies one synthetic Conventional Commit to slot `a`, runs the configured release tool, and asserts `version(a) == expected_0x` **and** `version(b) == BASELINE (0.1.0)` for every row of a shared expectation table. It already takes `--ecosystem NAME` and sources `ecosystems/$NAME.sh`; its `$REAL_TOML` argument is documented as release-plz-specific ("other ecosystems may ignore").
- `cases.tsv` — the shared S6 expectation table: `fix→0.1.1`, `feat→0.2.0`, `fix!→0.2.0`, `fix:+BREAKING footer→0.2.0`, `feat!→0.2.0` (0.x column; a 1.x column is carried but unasserted).
- `ecosystems/release-plz.sh` — the first adapter, implementing the 4-function interface `ecosystem::build_fixture / apply_commit / run_update / version`.

SMA-405 is the **Python adapter slice**: a *dormant* python-semantic-release (PSR) configuration for the two independent Python-native packages, plus a parity adapter so `run.sh --ecosystem python-semantic-release` reuses the same `cases.tsv` against PSR.

This matters because **PSR's 0.x defaults differ from release-plz's**. Two defaults in particular:

1. **`allow_zero_version` defaults to `false` since PSR v10** — left unset, PSR refuses 0.x entirely and the first version is `1.0.0` regardless of bump type.
2. **`major_on_zero` defaults to `true`** — even within 0.x, a breaking change bumps **major** (`0.1.0 → 1.0.0`), where release-plz (with `features_always_increment_minor = true`) bumps **minor** (`0.1.0 → 0.2.0`).

Surfacing and resolving that divergence is exactly why the parity gate exists (ADR-0011 S6).

## Goal

1. Add a **dormant** `[tool.semantic_release]` config to `paigasus-ml` and `paigasus-workflows` that honors the canonical 0.x contract.
2. Add `ci/release-parity/ecosystems/python-semantic-release.sh` implementing the existing 4-function adapter interface so the shared `run.sh` + `cases.tsv` drive it unchanged.
3. Wire the Python parity check into CI as a separate affected per-PR Moon task.

Non-goal: any live release workflow, registry publish, version activation, or touching the kernel/proto packages.

## Decision (what this branch delivers)

**Align PSR to the canonical contract (green).** Per ADR-0011 S6's "reconfigured to match", PSR is configured with `major_on_zero = false` + `allow_zero_version = true` so its 0.x classification is identical to release-plz's. Both ecosystems then assert the **same** `expected_0x` column of the **unchanged** `cases.tsv`. The divergence is caught *now* (during this implementation) and pinned by config; the gate's ongoing value is going **red** if the PSR config or a PSR upgrade ever drifts that classification.

Rejected: documenting PSR's native `breaking→1.0.0` as a known-exception column. PSR is configurable, and letting `paigasus-ml` classify a breaking change as `1.0.0` while a Rust crate classifies the same as `0.2.0` would create exactly the cross-language version drift ADR-0011's canonical contract exists to prevent.

Deliverables:

- `feat(release):` dormant `[tool.semantic_release]` in `py/packages/paigasus-ml/pyproject.toml` and `py/packages/paigasus-workflows/pyproject.toml`.
- `build(py):` PSR pinned as a uv dev-dependency (`py/pyproject.toml` `[dependency-groups] dev` + `uv.lock`).
- `feat(ci):` `ci/release-parity/ecosystems/python-semantic-release.sh` adapter + README note.
- `feat(ci):` a `release-parity-py` Moon task on the `repo` project, added to the `moon ci` target list.

**No changes** to `ci/release-parity/run.sh` or `ci/release-parity/cases.tsv` — both are already ecosystem-agnostic.

## Design

### 1. Dormant PSR config (S4)

Each package's `pyproject.toml` gains a `[tool.semantic_release]` table carrying the **classification-relevant** keys:

- `major_on_zero = false` — breaking-in-0.x → **minor** (the key alignment setting; clamps the bump via `min(level_bump, MINOR)`).
- `allow_zero_version = true` — stay in the 0.x regime (required explicitly: PSR v10 default is `false`).
- `version_toml = ["pyproject.toml:project.version"]` — PSR reads/writes the version in `project.version`.
- `tag_format = "paigasus-ml-v{version}"` (resp. `paigasus-workflows-v{version}`) — per-package tag namespace so the two packages never collide when activated.
- Default angular/conventional commit parser (no override) — maps `feat`→minor, `fix`→patch, `!` / `BREAKING CHANGE:` footer → breaking.

Dormancy is concrete and verifiable: the config exists and is valid, but **no workflow triggers PSR on push to `main`** (none is added), and the real packages **stay at `0.0.0`** — the `0.0.0 → 0.1.0` first activation is E-activate's job, not this slice's. The only observable behavior added is the dry-run parity check, which mutates nothing outside its temp dir.

This config does double duty exactly like `rs/release-plz.toml`: it is the production classification setting **and** the source the parity fixture derives from (F3 below).

### 2. The adapter — and the one real divergence from release-plz (PSR has no path attribution)

release-plz is workspace-aware: one repo, two crates, commits attributed to crates **by changed file path** (SMA-385's bug class — which is why the release-plz harness tests attribution). **PSR has no path-based monorepo attribution**: it versions one package from *all* commits since that package's last matching tag, regardless of which files changed. So the single-repo / two-package shape cannot make slot `b` stay at baseline while slot `a` bumps.

**Resolution: each slot gets its own independent git repo inside the fixture.**

- `ecosystem::build_fixture dir _ignored_real_toml` — create two throwaway sub-repos, `$dir/a` and `$dir/b`. Each is a minimal Python package (`pyproject.toml` with `project.name`, `project.version = "0.1.0"`, and a derived `[tool.semantic_release]` per F3) plus a seed source file. For each: `git init`, set `user.email`/`user.name`, disable commit/tag gpg signing, `git add -A`, seed commit, then `git tag <pkg>-v0.1.0` matching its `tag_format`. (Mirrors release-plz.sh's fixture hygiene; baseline lives in both the tag and `project.version` so PSR resolves the current version either way.)
- `ecosystem::apply_commit dir slot subject footer` — append a line to slot's source file; `git -C "$dir/$slot"` add + commit (`-m subject`, plus `-m footer` unless footer is `-`). Only that slot's repo gets the commit.
- `ecosystem::run_update dir` — for **each** slot, compute the next version read-only via `semantic-release version --print` (run with CWD inside the slot repo) and write the captured value to a per-slot sentinel `$dir/$slot/.parity-next-version`. `--print` computes the next version with **no** file/git/build mutation (PSR logs go to stderr; capture stdout, trim whitespace). **Both** slots are run through PSR — slot `a` (with a commit) must yield the bumped version; slot `b` (no commit since its baseline tag) must yield the baseline, which genuinely exercises "PSR invents no release without a qualifying commit." Capture stderr and replay on real failure (mirrors release-plz.sh).
- `ecosystem::version dir slot` — read the per-slot sentinel `$dir/$slot/.parity-next-version` written by `run_update`. The sentinel is the read-side state of the `run_update`→`version` split (replacing release-plz.sh's manifest read); because `--print` mutates nothing, the fixture `pyproject.toml` stays at its derived baseline for inspection.

The shared `check_case` contract (`got_a == expected`, `got_b == 0.1.0`) holds. **Documented honest difference** (README + here): for release-plz, slot `b` staying at baseline tests **path→package attribution** (SMA-385); for PSR it tests that **PSR invents no release without a qualifying commit**. Path attribution is a release-plz/cargo concern PSR does not claim, so the PSR fixture isolates histories instead. `b` is genuinely run through PSR (§2 `run_update` runs `--print` on both slots) and its computed version read from PSR's own output — not a hardcoded constant — so the assertion is not a false-green cheat.

### 3. Fixture config derived from the real configs, with a cross-package equality guard (F3 + review F1/F2)

The fixture's `[tool.semantic_release]` is **not** hand-mirrored. `build_fixture` derives it from the **real** PSR configs and writes the derived classification keys into each fixture `pyproject.toml`. Two guards run before deriving:

1. **Presence + value guard (F2).** Both classification keys must be present, and `allow_zero_version` must be `true`. A missing key, or `allow_zero_version != true`, **fails loudly** — e.g. `"real PSR config lacks major_on_zero — parity would test stale settings"` and `"allow_zero_version must be true — PSR would leave 0.x and the breaking-row assertions become meaningless"`. (Without this, a forgotten `allow_zero_version` would silently push PSR to `1.0.0` and fail every breaking row in a confusing way, rather than with a clear message.) Mirrors release-plz.sh's `_derive_config` loud guard, extended to both keys.
2. **Cross-package equality guard (F1).** The harness greps the classification keys (`major_on_zero`, `allow_zero_version`) from **both** real configs — `paigasus-ml` *and* `paigasus-workflows` — and **fails loudly if they disagree** (e.g. `"paigasus-ml and paigasus-workflows PSR classification keys differ (major_on_zero: false vs true) — both must honor the canonical contract"`). Only after they agree are the (now unambiguous) values written into the fixture.

This closes a false-green: §5 lists **both** packages' `pyproject.toml` as task inputs, so editing `-workflows`'s `[tool.semantic_release]` re-runs `release-parity-py`. Without the equality guard, the fixture would derive from `-ml` only and pass green regardless of what `-workflows` now says — so a later `major_on_zero = true` (or dropped `allow_zero_version`) in `-workflows` would silently classify breaking changes as `1.0.0` at activation while the gate stayed green. The equality guard makes that edit go **red**. Both configs are authored together in this slice, so closing it now is nearly free (it is the SMA-398 F3 fixture-drift concern recursed one level — within the Py adapter, across two packages).

This guarantees the harness exercises production classification settings; flipping `major_on_zero` in either real config (or making them disagree) flows into the fixture automatically and re-runs the check via the task inputs in §5. The `tag_format`/`version_toml`/`project.version` in the fixture are set to the fixture's own slot identities and baseline (not copied), since those are fixture-structural, not classification knobs.

### 4. PSR install + binary resolution

PSR is a Python package → installed as a **uv dev-dependency** in `py/pyproject.toml` `[dependency-groups] dev` (bounded constraint like `ruff`/`pytest`/`basedpyright`), pinned in `py/uv.lock`.

The fixture lives in `/tmp`, outside the repo, where `uv run` cannot resolve the project (the same trap release-plz.sh documents for the proto shim). So the adapter resolves the **absolute `semantic-release` binary once, from `py/`**, and invokes that directly — mirroring how release-plz.sh resolves `RELEASE_PLZ_BIN` via `proto bin`. Resolution: `(cd "$repo/py" && uv run --frozen which semantic-release)` with a `command -v semantic-release` fallback. PSR then reads its config from each slot's CWD `pyproject.toml`.

### 5. CI wiring (separate affected per-PR task)

A **separate** `release-parity-py` Moon task on the `repo` project (alongside the existing `release-parity`), so the affected graph stays granular — editing Rust release config does not trigger Python parity and vice-versa:

```yaml
release-parity-py:
  description: 'Dry-run python-semantic-release over synthetic commits; assert commit->semver parity (SMA-405).'
  script: 'ci/release-parity/run.sh --ecosystem python-semantic-release'
  toolchain: 'system'
  inputs:
    - 'ci/release-parity/**/*'
    - 'py/packages/paigasus-ml/pyproject.toml'
    - 'py/packages/paigasus-workflows/pyproject.toml'
    - 'py/uv.lock'
    - '.prototools'
```

`py/uv.lock` in `inputs` means a PSR **pin bump re-runs the check** (the tool-drift-detection mechanism, matching SMA-398 §9's rationale for `.prototools`). Add `:release-parity-py` to the `moon ci` target list in `.github/workflows/ci.yml` (the `T=(...)` array). The task needs the proto/uv-managed `semantic-release` reachable after `moon setup` (same toolchain-availability mechanism SMA-361 uses for buf/pnpm/uv; `run_update` resolves the binary from `py/` per §4).

## Verification plan (on this branch's PR)

1. **Harness green for PSR:** `moon run repo:release-parity-py` (or `ci/release-parity/run.sh --ecosystem python-semantic-release`) builds the two-repo fixture, runs PSR over all S6 rows, asserts every `version(a) == expected_0x` and `version(b) == 0.1.0`. Exit 0.
2. **Negative control fails red:** `run.sh --ecosystem python-semantic-release --negative-control` (the `fix!`→wrong-`0.1.1` probe) reports red — proves the PSR adapter has teeth and does not silently return "no bump".
3. **Alignment is real:** confirm `fix!` → `0.2.0` and `fix:`+`BREAKING CHANGE:` footer → `0.2.0` (equal, ≠ plain `fix` → `0.1.1`) under PSR — i.e. `major_on_zero = false` is in force. Confirm `feat` → `0.2.0`.
4. **0.x floor honored:** confirm PSR does not jump to `1.0.0` on a breaking change (would mean `allow_zero_version`/`major_on_zero` not applied).
5. **Attribution-equivalent (b-stability):** confirm slot `b` stays at `0.1.0` across every case (no release without a qualifying commit).
6. **Derived config + guards (F3 / review F1/F2):** flipping `major_on_zero` to `true` in `paigasus-ml`'s real config makes the breaking rows go red (→ `1.0.0`), proving the fixture derives from the real file, not a hardcoded copy. Making `-ml` and `-workflows` **disagree** (flip `major_on_zero` in just one) **fails loudly** with the cross-package equality message — not a silent green. Dropping `allow_zero_version` (or setting it `false`) in either real config **fails loudly** before any case runs.
7. **Dormancy:** no workflow triggers PSR on push; both real packages remain `version = "0.0.0"`; no tag/changelog/commit produced on `main`.
8. **Affected wiring:** a PR touching only an unrelated file does **not** run `release-parity-py`; a PR touching `ci/release-parity/**`, either py package `pyproject.toml`, `py/uv.lock`, or `.prototools` **does**. A Rust-only release-config change runs `release-parity` but **not** `release-parity-py`.

## Acceptance-criteria mapping

| AC (SMA-405) | How satisfied |
|--------------|----------------|
| Dormant PSR config for `paigasus-ml` **and** `paigasus-workflows` only | §1 — `[tool.semantic_release]` in both packages; kernel/proto explicitly out of scope (ADR-0011 S1/S2). |
| Adapter `ci/release-parity/ecosystems/python-semantic-release.sh` implementing the existing interface | §2 — `build_fixture`/`apply_commit`/`run_update`/`version`; driven by unchanged `run.sh`. |
| Reuses the shared `cases.tsv` | §2 — `run.sh --ecosystem python-semantic-release` reads the same expectation table; `cases.tsv` unchanged. |
| Surfaces / resolves 0.x classification divergence (ADR-0011 S6) | Decision + §1 — PSR's `major_on_zero`/`allow_zero_version` defaults diverge; reconfigured to match; gate goes red on future drift. |
| Dormant per ADR-0011 S4 | §1, Verification #7 — config + parity check only, no live workflow, packages stay `0.0.0`. |

## Risks / to-verify during implementation

1. **`version --print` output shape.** Confirm `semantic-release version --print` emits exactly the next version string on **stdout** (PSR logs to stderr), so the sentinel capture is clean. Trim whitespace and assert the captured value matches `^[0-9]+\.[0-9]+\.[0-9]+$`. If `--print` proves unworkable on the pinned PSR, fall back to write-then-read: `version --no-commit --no-tag --no-push --no-changelog --skip-build` writes `version_toml`, then `ecosystem::version` greps `project.version` (the original symmetric-with-release-plz approach).
2. **`--print` on a no-bump slot (slot `b`).** Confirm `--print` on a slot with no qualifying commit since its baseline tag prints the **current** version (`0.1.0`) on stdout with exit 0. If the pinned PSR instead errors or prints empty on a no-bump slot, fall back: treat a no-qualifying-commit slot (git-log `<tag>..HEAD` empty) as baseline — the git-log guard, demoted to a fallback for the no-bump-output uncertainty rather than the primary path, so slot `b` stays a genuine test when PSR cooperates.
3. **Current-version resolution.** Confirm PSR resolves the fixture baseline from the `<pkg>-v0.1.0` tag and/or `project.version = "0.1.0"` (both seeded). If PSR requires the tag specifically, the build_fixture tag is already present.
4. **Binary resolution from `/tmp`.** Confirm `(cd py && uv run --frozen which semantic-release)` yields an absolute binary that runs correctly with CWD inside `/tmp` and reads the slot's `pyproject.toml`. Adjust resolution if `uv run` insists on syncing the fixture.
5. **uv dev-dependency availability in CI.** Confirm `semantic-release` is installed/reachable after `moon setup` + `uv` install in the CI job (same path SMA-361 set up for the py toolchain).
6. **PSR pin.** Pin the exact latest-stable PSR (v10+) at implementation; record it in `uv.lock`.

## Out of scope

- Kernel/proto Python packages — `paigasus-kernel` Py is a **maturin** byproduct of the PyO3 binding; `paigasus-proto` Py is a **buf-codegen** byproduct of `contracts/` (ADR-0004). Both inherit their version from the Rust/contract source (ADR-0011 S1/S2), so neither is governed by PSR.
- Active PSR release workflow, changelog commits, tag-cutting, registry publish — E-activate / gated on SMA-378.
- First-activation `0.0.0 → 0.1.0` — E-activate.
- The 1.x expectation column — staged in `cases.tsv`, unasserted until the 1.0 transition.
- TS adapter (semantic-release for `@paigasus/sdk`/`@paigasus/ui`) — E4.
