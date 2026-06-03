# SMA-405 — python-semantic-release dormant config + Python semver-parity adapter

**Status:** Designed (brainstorming complete 2026-06-03)
**Date:** 2026-06-03
**Linear:** [SMA-405](https://linear.app/smaschek/issue/SMA-405/ci-python-semantic-release-dormant-config-py-semver-parity-adapter)
**Branch:** `feature/sma-405-ci-python-semantic-release-dormant-config-py-semver-parity`
**Targets:** `main` (currently `98bee2d`).
**References:** SMA-398 (parent — strategy ADR + the tool-agnostic Rust parity slice this builds on; spec `docs/superpowers/specs/2026-06-02-sma-398-release-tooling-strategy-and-rust-parity-design.md`); **ADR-0011** (Polyglot versioning & release strategy, Notion) — specifically **S1/S2** (kernel/proto Py packages are maturin byproducts of the Rust crate → out of scope here; `paigasus-ml`/`paigasus-workflows` are independent Py-native packages → in scope), **S4** (dormant: config + parity check only, no live workflow), **S6** (canonical commit→semver contract pinned to 0.x; "any tool whose 0.x defaults disagree is reconfigured to match or its divergence is documented as a known exception"). This issue is the spun-out **E3** from the SMA-398 spec.

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
- `ecosystem::run_update dir` — for **each** slot, if there are commits since its baseline tag (`git -C "$dir/$slot" log <tag>..HEAD` non-empty), run PSR there: `semantic-release version --no-commit --no-tag --no-push --no-changelog --skip-build`. This writes the bumped `project.version` into the slot's `pyproject.toml` with no git/build side-effects. The git-log guard means slot `b` (no commits) is skipped entirely, sidestepping any reliance on PSR's no-op exit behavior. Capture output and replay on real failure (mirrors release-plz.sh).
- `ecosystem::version dir slot` — grep `project.version` from `$dir/$slot/pyproject.toml` (`grep -m1 -E '^version[[:space:]]*='` within `[project]`, `sed` out the quoted value), mirroring release-plz.sh's manifest read.

The shared `check_case` contract (`got_a == expected`, `got_b == 0.1.0`) holds. **Documented honest difference** (README + here): for release-plz, slot `b` staying at baseline tests **path→package attribution** (SMA-385); for PSR it tests that **PSR invents no release without a qualifying commit**. Path attribution is a release-plz/cargo concern PSR does not claim, so the PSR fixture isolates histories instead. `b` is a real package whose version is genuinely read — not a hardcoded constant, so it is not a false-green cheat.

### 3. Fixture config derived from the real config (F3)

The fixture's `[tool.semantic_release]` is **not** hand-mirrored. `build_fixture` reads the **real** PSR config (canonical source: `py/packages/paigasus-ml/pyproject.toml`), greps the classification-relevant keys (`major_on_zero`, `allow_zero_version`), and writes them into each fixture `pyproject.toml`. If `major_on_zero` is absent it **fails loudly** ("real PSR config lacks major_on_zero — parity would test stale settings"), mirroring release-plz.sh's `_derive_config` guard. This guarantees the harness exercises production classification settings; flipping `major_on_zero` in the real config flows into the fixture automatically (and re-runs the check via the task inputs in §5). The `tag_format`/`version_toml`/`project.version` in the fixture are set to the fixture's own slot identities and baseline (not copied), since those are fixture-structural, not classification knobs.

> The two real packages (`-ml`, `-workflows`) carry **identical** `[tool.semantic_release]` classification keys. `paigasus-ml` is the canonical derivation source; a divergence in `paigasus-workflows`'s keys would not be caught by the fixture. Acceptable for this dormant slice (both authored together here); an optional cross-check guard can be added later if drift becomes a concern.

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
6. **Derived config (F3):** flipping `major_on_zero` to `true` in `paigasus-ml`'s real config makes the breaking rows go red (→ `1.0.0`), proving the fixture derives from the real file, not a hardcoded copy.
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

1. **PSR version-write under `--no-commit`.** Confirm `semantic-release version --no-commit --no-tag --no-push --no-changelog --skip-build` **writes** the bumped `project.version` into `version_toml` (the file write is part of the version step; `--no-commit` only skips the git commit). If a pinned PSR version does not write under `--no-commit`, fall back to `version --print` captured into a per-slot sentinel read by `ecosystem::version`.
2. **No-op behavior for slot `b`.** The git-log guard in `run_update` skips slot `b`; if instead PSR is run on `b`, confirm it exits 0 (not `--strict`) and leaves `project.version` at `0.1.0`.
3. **Current-version resolution.** Confirm PSR resolves the fixture baseline from the `<pkg>-v0.1.0` tag and/or `project.version = "0.1.0"` (both seeded). If PSR requires the tag specifically, the build_fixture tag is already present.
4. **Binary resolution from `/tmp`.** Confirm `(cd py && uv run --frozen which semantic-release)` yields an absolute binary that runs correctly with CWD inside `/tmp` and reads the slot's `pyproject.toml`. Adjust resolution if `uv run` insists on syncing the fixture.
5. **uv dev-dependency availability in CI.** Confirm `semantic-release` is installed/reachable after `moon setup` + `uv` install in the CI job (same path SMA-361 set up for the py toolchain).
6. **PSR pin.** Pin the exact latest-stable PSR (v10+) at implementation; record it in `uv.lock`.

## Out of scope

- Kernel/proto Python packages (`paigasus-kernel`, `paigasus-proto`) — maturin byproducts of the Rust crate, versioned by the Rust release (ADR-0011 S1/S2).
- Active PSR release workflow, changelog commits, tag-cutting, registry publish — E-activate / gated on SMA-378.
- First-activation `0.0.0 → 0.1.0` — E-activate.
- The 1.x expectation column — staged in `cases.tsv`, unasserted until the 1.0 transition.
- TS adapter (semantic-release for `@paigasus/sdk`/`@paigasus/ui`) — E4.
- A cross-check guard asserting `-ml` and `-workflows` PSR classification keys match — optional future hardening (§3 note).
