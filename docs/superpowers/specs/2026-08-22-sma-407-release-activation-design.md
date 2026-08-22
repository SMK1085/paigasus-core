# SMA-407 — Release activation: `0.1.0` floor, kernel/proto lockstep, live release workflows

**Status:** Designed (brainstorming complete 2026-08-22)
**Date:** 2026-08-22
**Linear:** [SMA-407](https://linear.app/smaschek/issue/SMA-407/release-activation-000-010-floor-kernelproto-lockstep-wiring-live)
**Branch:** `feature/sma-407-release-activation-000-010-floor-kernelproto-lockstep-wiring`
**Targets:** `main` (currently `14b8603`).
**References:** ADR-0011 (polyglot versioning & release strategy — S1 hybrid lockstep, S3 `0.1.0` floor + tool owns every tag, S4 dormant-until-real, S5 file-path attribution, S6 canonical contract); ADR-0005 (one kernel, many bindings); ADR-0006 (open-core dependency edge); ADR-0010 (release tooling); SMA-398 (the dormant config + parity harness this activates); SMA-376 (kernel crates.io metadata + the `release = false` guard this removes); SMA-378 (PyPI metadata); SMA-388 (`paigasus-proto` publish flip — a declared blocker, folded in here); SMA-385 (the Helikon manual-tag trap); SMA-307 (the two-job release-plz pattern); SMA-419 / SMA-427 / SMA-428 (the three FFI bindings); SMA-529 (`repo:publish-metadata`); SMA-530 (the negative-control precedent); SMA-541 (the `T=(…)` array contract); SMA-553 (`repo:input-liveness`).

---

## 1. Problem

Every package in `paigasus-core` sits at the `0.0.0` stub floor with publishing structurally
blocked. `rs/release-plz.toml` carries `[workspace] release = false`; `repo:publish-metadata`
Check 3 holds that line in place precisely *because* a publishable crate is still at `0.0.0`.
No release workflow exists at all.

ADR-0011 calls this final step **E-activate** and flags it as the riskiest one: the first tag
and the first upload are irreversible, and a hand-placed tag permanently breaks release-plz's
bump tracking (the SMA-385 failure this whole strategy was designed around).

## 2. What activates, and what does not

ADR-0011 S4 gates activation on a package having **a real public API**. Measured against the
tree, only two families clear that bar:

| Family | Members with real content |
| --- | --- |
| **kernel** | PRN canonicalization + UUIDv7 minting, exposed across Rust, PyO3, napi and wasm |
| **proto** | generated code committed in all three languages |

`paigasus-ml`, `paigasus-workflows`, `@paigasus/sdk` and `@paigasus/ui` are 1–3 line stubs with
no public API. They **stay at `0.0.0`**, and `repo:publish-metadata` Check 3 keeps guarding them.

This has a useful consequence for the sub-1.0 lifecycle question ADR-0011's 2026-06-04 amendment
routed here (§9, decision **G**): `@paigasus/sdk` / `@paigasus/ui` cannot reach a breaking change
before they have an API to break, so the decision is **deferred with a recorded reason** rather
than guessed at blind.

### Out of scope, each with a reason

| Deferred | Reason |
| --- | --- |
| `@paigasus/kernel`, `@paigasus/proto` npm publish | No JS emit exists anywhere in `ts/` — every "build" task is `tsc --noEmit`. Publishing them means introducing a TypeScript build pipeline, a subsystem in its own right. Follow-up issue. |
| `paigasus-ml`, `paigasus-workflows`, `@paigasus/sdk`, `@paigasus/ui` | No public API (ADR-0011 S4). |
| Decision **G** (sdk/ui sub-1.0 lifecycle) | Unreachable while those packages are stubs. Follow-up issue. |
| Actual registry uploads | The release job ships gated (§6). Flipping it live is a separate, deliberate human act. |

## 3. The version model

Two release-plz `version_group`s, each with exactly one Cargo crate as source of truth.
`version_group` is a per-package field taking a group name; packages sharing a group are held at
matching versions. release-plz bumps `publish = false` crates too — `publish` governs only
whether `cargo publish` runs — which is what lets the binding crates track the kernel without
ever reaching crates.io.

### `version_group = "kernel"` — source of truth: `paigasus-kernel`

| Site | Format | Destination |
| --- | --- | --- |
| `rs/crates/libs/paigasus-kernel/Cargo.toml` | Cargo | **crates.io** |
| `rs/crates/bindings/paigasus-py-bindings/Cargo.toml` | Cargo | — (`publish = false`) |
| `rs/crates/bindings/paigasus-py-bindings/pyproject.toml` | PEP 621 | **PyPI** (maturin wheel) |
| `py/packages/paigasus-kernel/pyproject.toml` | PEP 621 | **PyPI** (pure-Python wrapper) |
| `rs/crates/bindings/paigasus-node-bindings/Cargo.toml` | Cargo | — (`publish = false`) |
| `rs/crates/bindings/paigasus-node-bindings/package.json` | npm | **npm** |
| `rs/crates/bindings/paigasus-wasm/Cargo.toml` | Cargo | — (`publish = false`) |
| `rs/crates/bindings/paigasus-wasm/package.json` | npm | **npm** |

### `version_group = "proto"` — source of truth: `paigasus-proto`

| Site | Format | Destination |
| --- | --- | --- |
| `rs/crates/libs/paigasus-proto/Cargo.toml` | Cargo | **crates.io** (publish flipped — SMA-388) |
| `py/packages/paigasus-proto/pyproject.toml` | PEP 621 | **PyPI** |

### Why proto's source of truth is the crate

ADR-0011 S1 says the proto family is "versioned to track the proto contract", and no contract
version exists anywhere today. It does not need to: the generated Rust already lives inside
`rs/crates/libs/paigasus-proto/src/generated`, so a `contracts/` change regenerates it, changes
the crate's files, and release-plz attributes the bump **by file path** — which is exactly
ADR-0011 S5. "Tracks the contract" is therefore realized structurally, with no new mechanism,
and the crate stays inside release-plz's model so the SMA-398 commit→semver parity gate keeps
covering it. This is recorded as an S1 clarification in the ADR-0011 amendment (§9).

Note a consequence: a comment-only `.proto` edit shifts the embedded `FILE_DESCRIPTOR_SET`, so
it *does* count as a change to the crate and *does* bump the proto family. That is correct
behaviour — the wire artifact changed — not a defect.

### The dependency-pin site

`py/packages/paigasus-kernel` declares `dependencies = ["paigasus-py-bindings"]` — **unpinned** —
and reaches the local crate through `[tool.uv.sources]`. That table is development-only metadata:
uv strips it from the built distribution, so the published wrapper would float against *any*
version of the bindings, including a future incompatible one. It must be stamped to
`paigasus-py-bindings==X.Y.Z`.

### Site tally — who owns what

| | Count | Owner |
| --- | --- | --- |
| Cargo manifests (kernel, py-bindings, node-bindings, wasm, proto) | 5 | **release-plz**, via `version_group` |
| Non-Cargo manifests (2 × `pyproject.toml` in the kernel group, 2 × `package.json`, 1 × `pyproject.toml` in the proto group) | 5 | **`--write`** |
| Dependency pin (`py/packages/paigasus-kernel` → `paigasus-py-bindings==X.Y.Z`) | 1 | **`--write`** |
| **Total checked by `--check`** | **11** | |

So `--write` owns six sites; `--check` verifies all eleven, including the five release-plz
writes — a gate that trusted release-plz to have done its half would not notice a
`version_group` that silently stopped applying.

npm needs no equivalent: `napi create-npm-dirs` / `napi artifacts` generate the per-platform
`optionalDependencies` from `package.json`'s own version at publish time.

## 4. Lockstep mechanism

release-plz has **no hook mechanism**. Its configuration surface offers `version_group`,
`publish`, `release`, `release_always`, `git_tag_enable` and the changelog/PR keys — there is no
`pre_release_hook` and no pre/post-release command of any kind. So everything Cargo cannot reach
is stamped by one script with three modes:

**`ci/version-lockstep/run.sh`**

| Mode | Behaviour |
| --- | --- |
| `--check` (default) | Reads each group's source-of-truth Cargo version, compares against all eleven sites, reds on any drift. This is the `repo:version-lockstep` Moon gate. |
| `--write` | Rewrites the six non-Cargo sites from the source of truth. Run by the release-PR job (§6). |
| `--negative-control` | Proves the checker can still report red, per the SMA-530 precedent. |

One implementation, two operating modes, so the writer and the checker cannot disagree about
what "in lockstep" means — the same argument that made `ci/publish-metadata/run.sh` own both its
assertion and its `--refresh-categories` path.

**Exit codes** follow the house convention: `0` pass, `1` the repo is wrong, `2` infrastructure
failed (a manifest missing or unparseable).

## 5. Blocker: SMA-388

`paigasus-proto` is still `publish = false` and SMA-388 — a declared blocker on this issue — is
in Backlog. Its stated precondition ("once generated code lands") is now met: generated code is
committed in all three languages. The flip is a two-line change and is folded into this work
rather than serialized behind a separate PR; SMA-388 is closed as delivered here.

## 6. Release workflow

A new `.github/workflows/release.yml` implementing release-plz's two-job pattern (SMA-307), split
so that the reversible half runs for real and the irreversible half ships complete but inert.

### `release-pr` — live

Runs on push to `main`. Opens or updates the rolling release PR, then runs
`ci/version-lockstep/run.sh --write` and commits the stamped manifests onto release-plz's PR
branch. Nothing is tagged, nothing is uploaded; the PR is a proposal a human reads.

This is what makes the split worth having: the classification path runs against **real merges**
long before anything is published, so the first live release is not also the first time the
machinery has ever executed.

### `release` — shipped, gated

Guarded by `if: vars.PAIGASUS_RELEASE_ENABLED == 'true'`. Cuts tags, publishes to crates.io,
uploads maturin wheels to PyPI, publishes npm. `prebuild.yml` gains the publish credentials it
already anticipates (its line 45 reads *"SMA-407 adds publish creds at activation"*) behind the
same variable.

Activation is then flipping one repository variable — after the release PR has been observed
proposing sane versions.

### Wheels only, no sdist

`rs/crates/bindings/paigasus-py-bindings/pyproject.toml` carries a standing caveat: a published
sdist would not carry `rs/.cargo/config.toml`, whose apple-darwin `-undefined dynamic_lookup`
link flags the `extension-module` cdylib needs to link on macOS. A consumer building from sdist
on macOS would fail. We therefore publish **wheels only** and never upload an sdist, which
closes that trap rather than deferring it again.

## 7. CI bookkeeping

The repo's gate ceremony is load-bearing and is part of this design, not an afterthought.

- **`repo:version-lockstep`** must appear in **both** `ci.yml`'s `T=(…)` array and CLAUDE.md's
  marker-delimited command — `ci/affected-graph/ci_targets.py` asserts the two agree, and that
  every `T` entry resolves to a CI-eligible task (SMA-541). Its `inputs` must satisfy
  `repo:input-liveness`: every declared glob has to match at least one tracked file (SMA-553).
- **`repo:publish-metadata`**: `EXPECTED_PUBLISHABLE` gains `paigasus-proto`. Check 0 is a
  strict-equality set, so this is mandatory, not optional.
- **The guard's guard.** Check 3 asserts that a publishable crate at `0.0.0` is release-blocked.
  Once the floor moves to `0.1.0` that check becomes **vacuously satisfied** — it stops holding
  anything in place, and `[workspace] release = false` (whose own comment charters SMA-407 to
  remove it) goes away with it. The safety therefore has to be re-founded somewhere, or it
  silently degrades into a line anyone can delete. It moves to the workflow: a new assertion in
  `ci/actionlint/run.sh` — which already asserts workflow-level properties in its check 8 —
  requiring that the `release` job's `if:` guard is present, references
  `PAIGASUS_RELEASE_ENABLED`, and is not defeated by a `continue-on-error:` or a discarded exit
  status. Same shape as the existing check 8, same escape-hatch discipline.

## 8. Testing

- **`ci/version-lockstep/run.sh --self-test`** with a fixture table covering: each of the eleven
  sites drifted individually; the dependency-pin site left unpinned; a malformed version string;
  a missing manifest (exit 2, not 1); and both groups drifting at once.
- **`--negative-control`** run before the real check in the Moon task, under an explicit
  `set -euo pipefail`, exactly as the three `repo:release-parity*` tasks do (SMA-530) — Moon does
  not enable errexit for `script:` blocks, so without it a failing control is masked by the
  passing real run.
- **The existing `repo:release-parity*` suite is unchanged.** It already asserts the
  commit→semver contract; nothing here alters classification.
- **End-to-end evidence** is the release PR the live job opens on the first merge to `main` —
  observed and read, not published.

## 9. The first-tag risk

This is the single genuine hazard, and the reason §6 splits at the job boundary.

The repo has **no release tags at all**. release-plz determines "what has been released" from
tags, so with none present it treats every commit in history as unreleased. Setting `0.1.0` in
the manifests does not guarantee release-plz proposes `0.1.0` — given the accumulated `feat:`
history it may well propose `0.2.0` or higher on its first run.

We find that out by **reading the PR it opens**, with the release job still gated and no tag cut.
Whatever it proposes is then a decision made with evidence in hand.

What we do **not** do, under any circumstance, is hand-place a `*-vX.Y.Z` tag to "seed" the
tracking. Manually created tags lack the metadata release-plz uses to track releases and
silently stop all future bumps — the Helikon SMA-385 failure, and the direct motivation for
ADR-0011 S3's "the tool owns every tag".

## 10. Documentation

An **ADR-0011 amendment** recording three things:

1. **S1 clarification** — proto's "versioned to track the proto contract" is realized
   structurally, via the generated code committed inside the crate plus S5 file-path attribution.
   No contract version is introduced.
2. **S4 activation shape** — the release-PR job runs live while the release job ships gated
   behind a repository variable, and the guard that was Check 3's `release = false` pairing moves
   into `ci/actionlint/run.sh`.
3. **Decision G deferred** — with the reason (the packages are stubs; the exception cannot bite
   until they have a public API), so a future reader finds a recorded decision rather than an
   unanswered question.

Two follow-up Linear issues: the TypeScript emit pipeline for `@paigasus/kernel` /
`@paigasus/proto`, and decision **G**.

## 11. Risks

| Risk | Mitigation |
| --- | --- |
| release-plz proposes a version above `0.1.0` on first run | The release job is gated; we read the PR before enabling anything (§9). |
| A stamped manifest drifts silently | `repo:version-lockstep` reds CI on any of the nine sites; the negative control proves it can still red. |
| The published Python wrapper floats against any bindings version | The dependency-pin site is stamped and gate-checked (§3). |
| A macOS consumer builds from sdist and fails to link | Wheels only; no sdist is ever uploaded (§6). |
| The release gate degrades into a deletable line | Its assertion moves into `ci/actionlint/run.sh` check 8 (§7). |
| `version_group` behaves differently than documented | Checked against release-plz's config reference during design (it is a per-package field taking a group name; `publish = false` crates are still bumped). Confirm against the pinned release-plz version at plan time. Fallback if it does not hold: `--write` stamps the binding crates' Cargo versions too, which needs no new mechanism — only a wider write set. |
