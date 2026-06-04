# SMA-406 — semantic-release dormant config + TypeScript semver-parity adapter

**Status:** Designed (brainstorming complete 2026-06-04)
**Date:** 2026-06-04
**Linear:** [SMA-406](https://linear.app/smaschek/issue/SMA-406/ci-semantic-release-dormant-config-ts-semver-parity-adapter)
**Branch:** `feature/sma-406-ci-semantic-release-dormant-config-ts-semver-parity-adapter`
**Targets:** `main` (currently `6f7f585`).
**References:** SMA-398 (grandparent — the strategy ADR + the tool-agnostic Rust parity slice; spec `docs/superpowers/specs/2026-06-02-sma-398-release-tooling-strategy-and-rust-parity-design.md`); SMA-405 (sibling — the python-semantic-release adapter this mirrors structurally; spec `docs/superpowers/specs/2026-06-03-sma-405-python-semantic-release-parity-design.md`); **ADR-0011** (Polyglot versioning & release strategy, Notion) — specifically **S1/S2** (the kernel/proto TS packages are **derived** artifacts: `paigasus-kernel` TS is a **napi/wasm** byproduct of the Rust binding, `paigasus-proto` TS is a **buf-codegen** byproduct of `contracts/` (ADR-0004) — both inherit the Rust/contract version and are out of semantic-release scope; `@paigasus/sdk`/`@paigasus/ui` are independent TS-native packages → in scope), **S4** (dormant: config + parity check only, no live workflow), **S6** (canonical commit→semver contract pinned to 0.x). This issue is the spun-out **E4** from the SMA-398 spec, and the last sibling of the SMA-405 PSR adapter.

> **ADR-0011 S6 amended by this slice (2026-06-04).** S6 *as originally written* mandated the canonical contract (breaking→minor-in-0.x) **uniformly**, with no escape hatch for a tool that cannot be configured to comply — the "reconfigure-to-match **or** document-as-known-exception" phrasing that SMA-405 used was never actually in the ADR (SMA-405 got away with it because PSR *aligned*, never invoking the exception branch). SMA-406 is the first slice to rely on the exception branch, so as part of this work ADR-0011 S6 is amended to add that clause explicitly and to record the semantic-release TS exception. The sub-1.0 **lifecycle** consequence (see Decision → "Production-lifecycle consequence") is routed to **SMA-407**. See ADR-0011 "Amendment — 2026-06-04 (SMA-406)".

## Context / problem

SMA-398 landed a **tool-agnostic, multi-slot dry-run semver-parity harness** under `ci/release-parity/`; SMA-405 added the second ecosystem (python-semantic-release). The harness consists of:

- `run.sh` — the ecosystem-agnostic core. It builds a disposable fixture, applies one synthetic Conventional Commit to slot `a`, runs the configured release tool, and asserts `version(a) == expected` **and** `version(b) == BASELINE (0.1.0)` for every row of a shared expectation table. It takes `--ecosystem NAME` and sources `ecosystems/$NAME.sh`.
- `cases.tsv` — the shared S6 expectation table: `fix→0.1.1`, `feat→0.2.0`, `fix!→0.2.0`, `fix:+BREAKING footer→0.2.0`, `feat!→0.2.0` (0.x column; a 1.x column is carried but unasserted).
- `ecosystems/release-plz.sh`, `ecosystems/python-semantic-release.sh` — two adapters implementing the 4-function interface `ecosystem::build_fixture / apply_commit / run_update / version`.

SMA-406 is the **TypeScript adapter slice**: a *dormant* [semantic-release](https://semantic-release.gitbook.io/) configuration for the two independent TS-native packages, plus a parity adapter so `run.sh --ecosystem semantic-release` reuses the same `cases.tsv` against semantic-release.

### Why this is NOT symmetric with SMA-405 (the central finding)

SMA-405 *reconfigured PSR to match* the canonical 0.x contract (green) because PSR has a **version-aware** knob: `major_on_zero = false` clamps a breaking change to **minor** *only while in 0.x*, and auto-disengages at ≥1.0 (where breaking must become major again).

**JS semantic-release has no version-aware equivalent.** Its classification is fixed by the commit-analyzer preset (default angular/conventional: `feat`→minor, `fix`→patch, `!`/`BREAKING CHANGE:`→major) and applied via `semver.inc(lastVersion, type)`. There is **no 0.x clamp**. From a `0.1.0` baseline:

| case (id) | commit | canonical `expected_0x` | semantic-release native | verdict |
|-----------|--------|-------------------------|-------------------------|---------|
| `fix` | `fix:` | 0.1.1 | 0.1.1 | agrees |
| `feat` | `feat:` | 0.2.0 | 0.2.0 | agrees |
| `fix-bang` | `fix!:` | 0.2.0 | **1.0.0** | **diverges** |
| `fix-footer` | `fix:` + `BREAKING CHANGE:` footer | 0.2.0 | **1.0.0** | **diverges** |
| `feat-bang` | `feat!:` | 0.2.0 | **1.0.0** | **diverges** |

Three of five rows diverge: every breaking change escapes 0.x straight to `1.0.0`. This is exactly the divergence the issue predicts ("under strict npm semver every 0.x bump is 'breaking' … the adapter will likely surface a documented divergence … capturing that is the point").

The **only** lever that would force alignment is `@semantic-release/commit-analyzer`'s `releaseRules: [{ breaking: true, release: 'minor' }]` — but that rule is **unconditional**: it would also (wrongly) clamp breaking→minor *after* 1.0, with no auto-disengage. That makes the clean "reconfigure to match" path PSR used **unavailable** for semantic-release; choosing it would require remembering to remove the rule at the 1.0 transition — a fragile manual step.

## Goal

1. Add a **dormant** semantic-release config to `@paigasus/sdk` and `@paigasus/ui` that uses the default conventional classification (native strict semver, **no clamp**) plus per-package path-filtering (an in-repo path-filter, not a third-party monorepo plugin).
2. Add `ci/release-parity/ecosystems/semantic-release.sh` implementing the existing 4-function adapter interface so the shared `run.sh` + `cases.tsv` drive it.
3. Add a **minimal, generic** `ecosystem::expected` seam to `run.sh` so an ecosystem can declare a documented divergence from `expected_0x`; the semantic-release module uses it, asserting `breaking→1.0.0` (red on drift).
4. Wire a `release-parity-ts` affected per-PR Moon task into CI.

Non-goal: any live release workflow, registry publish, version activation, or touching the kernel/proto packages.

## Decision (what this branch delivers)

**Document the divergence; do not force-align (the now-ratified ADR-0011 S6 "documented as a known exception" branch).** semantic-release keeps its native strict-semver classification. The adapter declares, via the new `ecosystem::expected` hook, that the breaking rows are expected to yield `1.0.0` (not the canonical `0.2.0`). The harness then **asserts that documented divergence** — green when semantic-release behaves exactly as documented, **red** when a semantic-release upgrade or a config edit changes the classification. The divergence is captured now (in code + README + the table above) and continuously guarded.

Rejected: aligning via `releaseRules: [{breaking: true, release: 'minor'}]`. It is version-blind (would clamp post-1.0 breaking changes to minor, violating semver and the post-activation contract), so unlike PSR's `major_on_zero` it is not a safe 0.x-only setting. Aligning would trade a clearly-documented, gate-asserted divergence for a fragile config that silently does the wrong thing after the 1.0 transition.

**Production-lifecycle consequence — routed to SMA-407 (the substantive cost, not just a gate assertion).** The dormant config does double duty as the *production* classification config (§1). So documenting-rather-than-aligning is not cost-free: at activation, native semantic-release means `@paigasus/sdk` and especially `@paigasus/ui` (shared React components, breaking-prone in early development) **leave 0.x on their first breaking change** (→ `1.0.0`), while the Rust (release-plz) and Python (PSR) packages stay in 0.x (breaking→minor). That is a *different sub-1.0 lifecycle* for the TS-native packages than the rest of the platform, and a deviation from S6's canonical 0.x posture. This slice deliberately surfaces it rather than silently embedding it: **(a)** ADR-0011 S6 is amended (2026-06-04) to add the documented-exception clause and record this exception; **(b)** the lifecycle decision itself — accept early 1.0 for `sdk`/`ui`, *or* adopt the version-blind `releaseRules` clamp with a tracked 1.0-removal step, *or* reconsider the TS tool — is **routed to SMA-407** (activation), where it actually bites. SMA-406 ships the native dormant config; SMA-407 owns the lifecycle call.

**Stay on semantic-release (ADR-0010 "revisit changesets" trigger, resolved here).** ADR-0010 chose semantic-release for TS "or changesets … revisit when scaffolding `ts/` release flows" — and scaffolding this dormant config *is* that moment. The trigger is resolved **in favor of semantic-release**: changesets is file-driven (explicit change-entry files), not commit-message-driven, so adopting it would make TS the one ecosystem with no commit→semver contract — breaking the cross-language Conventional-Commits parity the whole harness exists to enforce. The cost of staying (the canonical monorepo plugin `semantic-release-monorepo` is abandoned + ESM-broken) is paid not by a single-vendor fork but by an **in-repo path-filter** (next paragraph), which keeps semantic-release core while removing the fragile dependency entirely.

**Fixture exercises the same path-filter the real config ships (one repo, two package dirs).** Because the real dormant config needs per-package path isolation (so a `sdk` commit never bumps `ui`), and because the canonical third-party monorepo plugin is abandoned, that isolation is provided by a small **in-repo path-filter** (a local semantic-release plugin that restricts `analyzeCommits` to commits touching the package's directory, via `git log -- <dir>`, before delegating to `@semantic-release/commit-analyzer`; tag namespacing is a plain `tagFormat`). The fixture is a single git repo with two package dirs driven through that **same** in-repo path-filter — so slot `b` staying at baseline tests **path→package attribution**, the exact mechanism the real config ships. This parallels `release-plz.sh` (cargo path attribution, SMA-385) rather than `python-semantic-release.sh` (which used two isolated repos because PSR has no attribution to test). Primary approach; the maintained `@rimac-technology/semantic-release-monorepo` fork and `multi-semantic-release` are documented fallbacks (Risk 1).

Deliverables:

- `feat(release):` dormant semantic-release config (`.releaserc.json`) in `ts/packages/paigasus-sdk/` and `ts/packages/paigasus-ui/`.
- `feat(release):` the in-repo path-filter (a small local semantic-release plugin under `ts/`, e.g. `ts/tooling/semantic-release-path-filter.mjs`), referenced by both package configs and the fixture.
- `build(ts):` `semantic-release` + `@semantic-release/commit-analyzer` pinned as `ts/` pnpm dev-dependencies (`ts/package.json` + `ts/pnpm-lock.yaml`). No third-party monorepo plugin in the primary path.
- `feat(ci):` `ci/release-parity/ecosystems/semantic-release.sh` adapter + README section.
- `feat(ci):` the generic `ecosystem::expected` hook in `ci/release-parity/run.sh`.
- `feat(ci):` a `release-parity-ts` Moon task on the `repo` project, added to the `moon ci` target list.

**No changes** to `ci/release-parity/cases.tsv` — it stays the shared, unchanged expectation table.

## Design

### 1. Dormant semantic-release config (S4)

Each of `ts/packages/paigasus-sdk/` and `ts/packages/paigasus-ui/` gains a `.releaserc.json` carrying:

- `plugins: ["<in-repo-path-filter>"]` — the in-repo path-filter is the **only** `analyzeCommits` provider: it restricts the analyzed commits to those touching the package directory and then **delegates internally** to `@semantic-release/commit-analyzer`. Do **not** also list `@semantic-release/commit-analyzer` separately — semantic-release runs `analyzeCommits` for every listed plugin and takes the **max** release type, so a second, unfiltered analyzer would defeat the path filter. **No `releaseRules`** and the default conventional preset → native strict-semver classification (the documented divergence). No `@semantic-release/npm` / `@semantic-release/github` (those are publish/push plugins; activation and their tokens are SMA-407's job).
- `tagFormat` — per-package namespace (`@paigasus/sdk-v${version}`, resp. `@paigasus/ui-v${version}`) so the two packages never collide when activated. Set directly (a plain semantic-release option; no plugin needed for namespacing). (See Risk 5 on `@`/`/` in git refs.)
- `branches: ["main"]` — pin the release branch.

Dormancy is concrete and verifiable: the config exists and is valid, but **no workflow triggers semantic-release on push to `main`** (none is added), and the real packages **stay `private: true` and `version: "0.0.0"`** — the `0.0.0 → 0.1.0` first activation is SMA-407's job. The only observable behavior added is the dry-run parity check, which mutates nothing outside its temp dir.

This config does double duty exactly like `rs/release-plz.toml` and the PSR `pyproject.toml` tables: it is the production classification setting **and** the source the parity fixture derives its classification from (F3, §4). The path-filter wiring and `tagFormat` are *not* the classification knobs — `releaseRules`/`preset` are (§4).

### 2. The one `run.sh` change — a generic `expected` hook

SMA-405 left `run.sh` untouched because PSR was aligned (every row asserts `expected_0x`). Documenting a divergence needs a **generic, ecosystem-agnostic seam**, not a semantic-release special case baked into the core.

`run.sh` currently passes `$expected_0x` to `check_case` as the slot-`a` expectation. New behavior: after sourcing the ecosystem module, resolve the expectation through an optional hook:

```sh
# Default: the canonical 0.x expectation. An ecosystem MAY override it to assert a
# documented, intentional divergence (e.g. semantic-release's strict-semver breaking->major).
resolve_expected() { # id subject footer expected_0x expected_1x discr -> expected
  if declare -F ecosystem::expected >/dev/null; then
    ecosystem::expected "$@"
  else
    printf '%s' "$4"   # expected_0x
  fi
}
```

`release-plz.sh` and `python-semantic-release.sh` do **not** define `ecosystem::expected` → byte-for-byte identical behavior (regression-safe). The `semantic-release.sh` module defines it to encode strict semver:

```sh
# Strict npm semver (no 0.x clamp): any breaking marker -> major bump from baseline.
# This is the documented ADR-0011 S6 divergence; the gate goes red if semantic-release
# ever stops doing this (upgrade) or the real config starts clamping (F3 guard, §4).
ecosystem::expected() { # id subject footer expected_0x expected_1x discr
  local subject="$2" footer="$3" expected_0x="$4"
  if printf '%s' "$subject" | grep -qE '^[a-z]+(\([^)]*\))?!:' \
     || printf '%s' "$footer" | grep -q 'BREAKING CHANGE'; then
    printf '1.0.0'        # strict-semver major from the 0.1.0 baseline
  else
    printf '%s' "$expected_0x"
  fi
}
```

Encoding the *rule* (breaking → major) rather than a per-id lookup keeps the divergence self-documenting and correctly classifies any future breaking row added to `cases.tsv`. `1.0.0` is the major bump of the harness `BASELINE` (`0.1.0`); a comment ties the two together.

**1.0-transition note (do not let `1.0.0` become a stale constant).** The literal `1.0.0` is correct for *any* 0.x baseline (the major bump of `0.y.z` is always `1.0.0`), so it is right while only the `expected_0x` column is asserted. It becomes **wrong** once the staged `1.x` column is asserted (the major of `1.2.0` is `2.0.0`). The hook already receives `expected_1x`/`discr` (unused today), so the seam exists: at the 1.0 transition the hook must compute *major-of-baseline* (or consume `expected_1x`) in lockstep with `cases.tsv`. A comment in the hook records this so it is revisited, not silently shipped stale.

**Negative control unchanged.** `run.sh --negative-control` passes its own explicit wrong expectation (`fix!`→`0.1.1`) **directly** to `check_case` and is intentionally *not* routed through `resolve_expected`. For semantic-release the real result is `1.0.0` ≠ the fed `0.1.1` → the harness still reports red → the probe still proves the adapter computes a real version and compares it (it does not silently return "no bump"). The existing negative-control comment is release-plz-framed but its assertion (`red` on a deliberately wrong expectation) holds for every ecosystem.

### 3. The adapter — one repo, two package dirs, in-repo path-filtering

The four interface functions, fixture rooted at the `run.sh`-provided `mktemp` dir:

- **`ecosystem::build_fixture dir _ignored_real_toml`** — one git repo at `$dir` with two package dirs `$dir/a` and `$dir/b`. Each holds a minimal `package.json` (`name` = a slot-unique package name, `version = "0.1.0"`, `private: true`, `type: "module"`), a derived `.releaserc.json` (F3, §4), and a seed source file. Then: `git init` (pin `init.defaultBranch=main`), set `user.email`/`user.name`, disable commit/tag gpg signing, seed `git add -A` + commit, `git tag a-v0.1.0` + `git tag b-v0.1.0` (matching each slot's `tagFormat`), and add a placeholder `origin` remote (semantic-release reads `git config remote.origin.url`; the URL is never contacted). Mirrors the fixture hygiene in both existing adapters. The fixture `.releaserc.json` references the in-repo path-filter by absolute path; because the path-filter file lives under `ts/`, its own `import '@semantic-release/commit-analyzer'` resolves from `ts/node_modules` regardless of the fixture's `/tmp` cwd (§5).
- **`ecosystem::apply_commit dir slot subject footer`** — append a line to slot's source file *under that slot's dir* (so the commit's changed path is `a/...` or `b/...`); `git add -A` + commit (`-m subject`, plus `-m footer` unless footer is `-`). Single shared repo, single history; only the changed **path** distinguishes the slots.
- **`ecosystem::run_update dir`** — for **each** slot, compute the next version read-only via the semantic-release **JS API** through an in-repo runner (`node ts/tooling/semantic-release-next-version.mjs <slotDir>`, §5). The runner calls `semanticRelease({dryRun:true, ci:false}, {cwd: slotDir})` and prints `result.nextRelease.version` (or empty when `result === false`, i.e. no release). The in-repo path-filter restricts analysis to that slot dir, so slot `a` (a commit under `a/`) bumps and slot `b` (no commit under `b/` since its baseline tag) returns no release → baseline. Write the value to a per-slot sentinel `$dir/$slot/.parity-next-version` (empty → `BASELINE`). **Both** slots run through semantic-release, so `b` is a genuine assertion, not a hardcoded constant. Capture the runner's stderr and replay on real failure (mirrors both existing adapters). Using the JS API (not the CLI) means the version comes back structured — no human-readable-log scraping.
- **`ecosystem::version dir slot`** — read the per-slot sentinel `$dir/$slot/.parity-next-version` written by `run_update`. The sentinel is the read-side of the `run_update`→`version` split (dry-run mutates nothing, so the fixture `package.json` stays at its baseline for inspection).

The shared `check_case` contract (`got_a == expected`, `got_b == 0.1.0`) holds, with `expected` resolved through §2's hook. **Documented honest difference** (README + here): for release-plz, slot `b` staying at baseline tests **cargo path→package attribution** (SMA-385); for PSR it tests **"no release without a qualifying commit"** (PSR has no attribution); for semantic-release it tests **path→package attribution via the in-repo path-filter** — the exact mechanism the real `sdk`/`ui` config ships for per-package isolation. `b` is genuinely run through semantic-release, so the assertion is not a false-green cheat.

### 4. Fixture config derived from the real configs, with guards (F3)

The fixture's classification config is **not** hand-mirrored; `build_fixture` derives it from the **real** `sdk`/`ui` configs and writes the derived bits into each fixture `.releaserc.json`. Because semantic-release configs are JSON, read them with `node -p` (node is a Moon-managed toolchain, already on PATH in CI). Two guards run before deriving:

1. **No-clamp guard (presence/value analogue).** The classification-relevant bits are the commit-analyzer `preset` (default if absent) and `releaseRules`. The documented native divergence holds **only** while there is no clamp, so if either real config carries a `releaseRules` entry, **fail loudly**: e.g. `"real semantic-release config has commit-analyzer releaseRules — the documented breaking->1.0.0 divergence no longer holds; update the divergence table + ecosystem::expected"`. This is the loud-guard analogue of release-plz.sh's `_derive_config` and PSR's presence guard, inverted (here the *presence* of a knob is the failure, because native = absence).
2. **Cross-package equality guard.** Derive the classification bits from **both** `@paigasus/sdk` *and* `@paigasus/ui` and **fail loudly if they disagree** (e.g. `"@paigasus/sdk and @paigasus/ui semantic-release classification differs — both must honor the same contract"`). Only after they agree are the (now unambiguous) values written into the fixture.

This closes a false-green exactly as in SMA-405 §3: §5 lists **both** packages' configs as task inputs, so editing one re-runs `release-parity-ts`; without the equality guard the fixture would derive from one package only and pass green regardless of what the other now says. The path-filter wiring and `tagFormat` in the fixture are set to the fixture's own slot identities and baseline (not copied) — those are fixture-structural, not classification knobs (same split PSR used for `tag_format`/`version_toml`).

**Why this matters with the documented-divergence strategy:** `ecosystem::expected` (§2) hardcodes "breaking → 1.0.0". If someone later *aligns* the real config (adds a `releaseRules` clamp), guard (1) fails loudly *before* any case runs — forcing the divergence table + hook to be revisited, instead of a silent stale green. The gate thus stays correct whether semantic-release drifts (upgrade changes behavior → a case goes red) or the config drifts (clamp added → guard 1 fails loud).

### 5. Invocation + module resolution (JS API runner)

semantic-release and `@semantic-release/commit-analyzer` are **Node** tooling → installed as `ts/` pnpm dev-dependencies (`ts/package.json`), pinned in `ts/pnpm-lock.yaml`. CI already runs `pnpm --dir ts install --frozen-lockfile`, so they are present before `moon ci`.

The fixture lives in `/tmp`, outside `ts/`. Rather than invoke the CLI from there (which would need absolute-binary resolution *and* cwd-relative ESM plugin resolution gymnastics), the adapter calls a tiny in-repo runner, `ts/tooling/semantic-release-next-version.mjs`, that uses the semantic-release **JS API** (`semanticRelease({dryRun:true, ci:false}, {cwd: slotDir})`). Module resolution then falls out cleanly, because ESM resolves a module's bare imports relative to **that module's own location**, not the process cwd:

- **`semantic-release`** — the runner lives under `ts/`, so `import 'semantic-release'` resolves from `ts/node_modules`.
- **the in-repo path-filter** — referenced by **absolute path** in the (fixture or real) `.releaserc.json`, so semantic-release loads it regardless of cwd.
- **`@semantic-release/commit-analyzer`** — imported *by* the path-filter, which also lives under `ts/`, so it too resolves from `ts/node_modules`.

No symlinking, no `require.resolve`, no CLI-log parsing: the runner prints `result.nextRelease.version` to stdout (semantic-release's own logs are routed to stderr), or empty when `result === false`. This supersedes the earlier CLI+parse sketch and folds away the old binary-resolution and output-parsing risks (see Risks 2–3).

### 6. CI wiring (separate affected per-PR task)

A **separate** `release-parity-ts` Moon task on the `repo` project (alongside `release-parity` and `release-parity-py`), so the affected graph stays granular — editing Rust or Python release config does not trigger the TS parity check and vice-versa:

```yaml
release-parity-ts:
  description: 'Dry-run semantic-release over synthetic commits; assert commit->semver parity + documented 0.x divergence (SMA-406).'
  script: 'ci/release-parity/run.sh --ecosystem semantic-release'
  toolchain: 'system'
  inputs:
    - 'ci/release-parity/**/*'
    - 'ts/packages/paigasus-sdk/.releaserc.json'
    - 'ts/packages/paigasus-ui/.releaserc.json'
    - 'ts/tooling/semantic-release-path-filter.mjs'   # the in-repo path-filter (also exercised by the fixture)
    - 'ts/pnpm-lock.yaml'
    - '.prototools'
```

`ts/pnpm-lock.yaml` in `inputs` means a `semantic-release` / `@semantic-release/commit-analyzer` **pin bump re-runs the check** — the tool-drift-detection mechanism (matching SMA-405 §5's `py/uv.lock` and SMA-398 §9's `.prototools` rationale): an upgrade that changes classification goes red here. Add `:release-parity-ts` to the `moon ci` target list in `.github/workflows/ci.yml` (the `T=(…)` array). The task needs `node` + the installed semantic-release binary/plugins reachable after `moon setup` + the existing pnpm install (same toolchain-availability path SMA-361/SMA-405 established).

## Verification plan (on this branch's PR)

1. **Harness green for semantic-release:** `moon run repo:release-parity-ts` (or `ci/release-parity/run.sh --ecosystem semantic-release`) builds the one-repo/two-dir fixture, runs semantic-release over all S6 rows, asserts every `version(a) == resolve_expected(row)` and `version(b) == 0.1.0`. Exit 0.
2. **Divergence is real and asserted:** confirm `fix!`, `fix:`+`BREAKING CHANGE:` footer, and `feat!` each yield `1.0.0` (not the canonical `0.2.0`), and `fix`→`0.1.1`, `feat`→`0.2.0` agree. The breaking rows pass *because* the hook asserts `1.0.0`.
3. **Negative control fails red:** `run.sh --ecosystem semantic-release --negative-control` (the `fix!`→wrong-`0.1.1` probe) reports red — proves the adapter computes a real version and does not silently return "no bump".
4. **Drift would go red (teeth):** temporarily editing `ecosystem::expected` to assert `0.2.0` for the breaking rows makes them fail (tool returns `1.0.0`) — demonstrating the gate catches a future classification change. (Revert.)
5. **Path attribution (b-stability):** confirm slot `b` stays at `0.1.0` across every case — the in-repo path-filter excluded slot `a`'s commit by path. Inverting the touched path (commit under `b/`) would bump `b` and leave `a` at baseline (spot-check during implementation).
6. **F3 derive + guards:** removing/altering the real `sdk` config's classification flows into the fixture (the fixture is derived, not hardcoded). Adding a `releaseRules` clamp to either real config **fails loudly** (guard 1) before any case runs. Making `sdk` and `ui` disagree **fails loudly** (guard 2).
7. **Dormancy:** no workflow triggers semantic-release on push; both real packages remain `private: true` + `version = "0.0.0"`; no tag/changelog/commit produced on `main`.
8. **Affected wiring:** a PR touching only an unrelated file does **not** run `release-parity-ts`; a PR touching `ci/release-parity/**`, either package's `.releaserc.json`, `ts/pnpm-lock.yaml`, or `.prototools` **does**. A Rust- or Python-only release-config change runs `release-parity`/`release-parity-py` but **not** `release-parity-ts`.

## Acceptance-criteria mapping

| AC (SMA-406) | How satisfied |
|--------------|----------------|
| Dormant semantic-release config for `@paigasus/sdk` **and** `@paigasus/ui` only | §1 — `.releaserc.json` in both packages; kernel/proto explicitly out of scope (ADR-0011 S1/S2). |
| Monorepo path-filtering | §1, §3, Risk 1 — primary is an **in-repo path-filter** (a small local semantic-release plugin restricting commits by package dir; no third-party monorepo plugin), with `@rimac-technology/semantic-release-monorepo` and `multi-semantic-release` as documented fallbacks (the abandoned `semantic-release-monorepo`/pmowrer is rejected). ADR-0010's "revisit changesets" trigger resolved in favor of semantic-release (Decision). |
| Adapter `ci/release-parity/ecosystems/semantic-release.sh` implementing the existing interface | §3 — `build_fixture`/`apply_commit`/`run_update`/`version`; driven by `run.sh`. |
| Reuses the shared `cases.tsv` | §2/§3 — same expectation table; `cases.tsv` unchanged; the divergence is expressed via the generic `ecosystem::expected` hook, not a new column. |
| Surfaces the 0.x classification divergence (ADR-0011 S6) | Decision + §2 + Context table — breaking→`1.0.0` documented and gate-asserted (the "documented as a known exception" branch of S6). |
| Dormant per ADR-0011 S4 | §1, Verification #7 — config + parity check only, no live workflow, packages stay `0.0.0`. |

## Risks / to-verify during implementation

1. **Path-filter mechanism.** The naive third-party plugin `semantic-release-monorepo` (pmowrer) is abandoned (v8.0.2, ~2 years stale) and throws `ERR_REQUIRE_ESM` against modern ESM semantic-release, so it is rejected. **Primary: an in-repo path-filter** — a small local semantic-release plugin that, in `analyzeCommits` (and `generateNotes` if notes are ever enabled), restricts `context.commits` to those touching the package's directory before delegating to `@semantic-release/commit-analyzer`; tag namespacing is a plain `tagFormat`. This keeps semantic-release core, removes the single-vendor-fork dependency from a public-repo supply chain (ADR-0010 durability), and trivializes `/tmp` resolution (absolute path, §5). To verify during implementation: the exact shape semantic-release exposes for `context.commits` and whether the filter must obtain changed files via its own `git log --name-only -- <dir>` (semantic-release does not attach file lists to commits by default). **Fallback A:** `@rimac-technology/semantic-release-monorepo` (maintained ESM/TS fork, same per-package wrapper model) if the in-repo filter proves brittle against the pinned semantic-release internals. **Fallback B:** `multi-semantic-release` (whole-repo orchestration model; requires reshaping `run_update` to a single invocation). Lock the choice in the implementation plan after a spike against the pinned version.
2. **Module resolution from `/tmp` (largely resolved by the JS-API runner, §5).** Because the runner and the path-filter both live under `ts/`, `semantic-release` and `@semantic-release/commit-analyzer` resolve from `ts/node_modules` natively — no symlink/`require.resolve`. The only remaining check: `build_fixture` must write the path-filter's **absolute** path into each fixture `.releaserc.json` (the real package configs use a path relative to the package dir, which is the cwd at activation — verify semantic-release resolves that relative form).
3. **JS-API dry-run behavior (replaces the old output-parsing risk).** Confirm the JS API in `dryRun` returns a truthy `result` with `result.nextRelease.version` for a qualifying commit, and a **falsy** `result` (`false`) for a slot with no qualifying commit since its baseline tag (→ baseline). Confirm it needs no network/token with the minimal single-plugin config and the placeholder `origin` remote, and that `ci:false` is honored. (Verified empirically in the plan's runner task before the full adapter is wired.)
4. **Offline / no-token / no-CI.** With a minimal `plugins` set (no `@semantic-release/npm` / `@semantic-release/github`), dry-run should perform no registry/token verification. Confirm the JS-API `{dryRun:true, ci:false}` run needs no network and that semantic-release does not error on the placeholder `origin` remote.
5. **Namespaced tag validity.** Confirm `@paigasus/sdk-v${version}` (containing `@` and `/`) compiles to a valid git ref for the *real* config; if git rejects it, sanitize (`paigasus-sdk-v${version}`). The **fixture** uses simple slot tags (`a-v0.1.0`/`b-v0.1.0`) regardless.
6. **semantic-release version pin.** Pin the exact latest-stable `semantic-release` at implementation; record it in `ts/pnpm-lock.yaml`. Note the pinned major when documenting Risk 1's compat check.

## Out of scope

- Kernel/proto TypeScript packages — `@paigasus/kernel` TS is a **napi/wasm** byproduct of the Rust binding; `@paigasus/proto` TS is a **buf-codegen** byproduct of `contracts/` (ADR-0004). Both inherit their version from the Rust/contract source (ADR-0011 S1/S2), so neither is governed by semantic-release.
- Active semantic-release workflow, changelog commits, tag-cutting, registry publish, the `@semantic-release/npm`/`github` plugins, and flipping `private: false` — SMA-407 / gated on activation.
- First-activation `0.0.0 → 0.1.0` — SMA-407.
- **The sub-1.0 lifecycle decision for `sdk`/`ui`** (accept early 1.0 vs. a version-blind `releaseRules` clamp with a tracked 1.0-removal step vs. a tool switch) — surfaced by this slice (Decision, "Production-lifecycle consequence") and **routed to SMA-407** (see the SMA-407 comment + ADR-0011 Amendment 2026-06-04). This slice ships only the native dormant config.
- The `1.x` expectation column — staged in `cases.tsv`, unasserted until the 1.0 transition (see §2's 1.0-transition note for the `ecosystem::expected` change required then).
