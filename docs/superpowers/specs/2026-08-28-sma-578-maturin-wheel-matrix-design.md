# SMA-578 — Release activation C: maturin cross-platform wheel matrix for `paigasus-py-bindings`

**Status:** Draft (brainstorming 2026-08-28)
**Date:** 2026-08-28
**Linear:** [SMA-578](https://linear.app/smaschek/issue/SMA-578/release-activation-c-maturin-cross-platform-wheel-matrix-for-paigasus)
— child of [SMA-407](https://linear.app/smaschek/issue/SMA-407), unblocked by SMA-576 (Done),
blocking SMA-580. **Folds in [SMA-556](https://linear.app/smaschek/issue/SMA-556)** (§7.2).
**Branch:** `feature/sma-578-release-activation-c-maturin-cross-platform-wheel-matrix-for`
**Targets:** `main` (currently `3f23758`).
**Umbrella design:** `docs/superpowers/specs/2026-08-22-sma-407-release-activation-design.md`
— this document implements **§7's PyPI half** and **corrects its §7/M3** (see §2 below).
**References:** ADR-0005 (one kernel, many bindings); ADR-0011 (S3 the tool owns every tag,
S4 dormant-until-real); SMA-419 (the PyO3/maturin binding); SMA-428 (the napi matrix — the
shape and the precedent); SMA-378 (`uv_build` license-files); SMA-529/SMA-530 (negative
controls); SMA-541/SMA-553 (gate bookkeeping); SMA-542 (guard-the-guard); SMA-576; SMA-577.

---

## 1. Problem

The kernel family sits at the `0.1.0` floor and `repo:version-lockstep` pins
`py/packages/paigasus-kernel` (the Python face) `==` to `paigasus-py-bindings` (the
maturin/PyO3 wheel). Neither can be installed from PyPI today, and **no wheel matrix exists
anywhere in the repo**. `prebuild.yml`'s six-leg matrix builds the *napi addon*; there is no
cibuildwheel and no maturin-action.

A single-runner build yields one wheel. Under the umbrella design's "wheels only, no sdist"
answer there is no fallback, so `pip install paigasus-kernel` would fail outright on macOS,
Windows and linux-aarch64.

Two secondary gaps ride along:

- `rs/crates/bindings/paigasus-py-bindings/pyproject.toml` carries essentially no PyPI
  metadata, and the crate dir has no `LICENSE` or `README.md`. **Measured:** the built wheel's
  `METADATA` is **88 bytes**. Nothing gates PyPI metadata the way `repo:publish-metadata`
  gates crates.io (umbrella §14 Q8).
- **The release guard is currently unfounded.** Umbrella §9 records that
  `repo:publish-metadata` Check 3 — "a publishable crate at `0.0.0` must be release-blocked" —
  goes *vacuously satisfied* at the `0.1.0` floor (`ci/publish-metadata/run.sh:177` skips the
  block when no publishable crate is at `0.0.0`), and that a replacement assertion must move
  into `ci/actionlint/run.sh`. SMA-576 listed that in its scope but **could not implement it**:
  there was no `release` job to guard. Verified on `3f23758` — `ci/actionlint/run.sh` contains
  no `PAIGASUS_RELEASE_ENABLED` assertion and `SELF_TEST_COUNT` is still 9. This issue creates
  the job, so it inherits the obligation.

## 2. The measurement that redirects the design

The `pyproject.toml` caveat, umbrella §7's **review M3**, and §15's risk row all rest on one
claim: *a published sdist would not carry `rs/.cargo/config.toml`, whose apple-darwin
`-undefined dynamic_lookup` flags the `extension-module` cdylib needs to link — so a macOS
consumer building from sdist fails.*

**The observation is true; the conclusion is false.** Measured on macOS (darwin 25.6.0,
maturin 1.9.6, cargo 1.95):

| Probe | Command (run from the **repo root**, where `rs/.cargo/config.toml` is *not* on cargo's upward walk) | Result |
| --- | --- | --- |
| Build | `maturin build -m rs/crates/bindings/paigasus-py-bindings/Cargo.toml` | **exit 0** — `paigasus_py_bindings-0.1.0-cp312-abi3-macosx_11_0_arm64.whl` |
| **Control** | `cargo build -p paigasus-py-bindings --manifest-path rs/Cargo.toml` | **fails** — `ld: symbol(s) not found for architecture arm64`, undefined `__Py_IncRef`, `__Py_NoneStruct` |
| sdist | `maturin sdist -m …/Cargo.toml` | **exit 0** |

The control is what makes the attribution sound: identical working directory, identical
manifest, identical absence of the config file — only the tool differs. **maturin injects the
darwin link arguments itself.** `rs/.cargo/config.toml` exists so that a *non-maturin*
`cargo build` links, which is precisely what its own comment says ("These flags let
`cargo build` link the extension on macOS **WITHOUT maturin**"). It is irrelevant to packaging.

The sdist is also structurally sound. Its contents:

```
paigasus_py_bindings-0.1.0/crates/libs/paigasus-kernel/{Cargo.toml,LICENSE,README.md,src/…,tests/…}
paigasus_py_bindings-0.1.0/crates/bindings/paigasus-py-bindings/{Cargo.toml,moon.yml,paigasus_py_bindings.pyi,src/lib.rs}
paigasus_py_bindings-0.1.0/{Cargo.lock,Cargo.toml,pyproject.toml,PKG-INFO}
```

maturin **vendors the workspace path dependency** (`crates/libs/paigasus-kernel/`) and rewrites
the workspace manifest, and ships `Cargo.lock`. It also ships **`moon.yml`** — the identical
repo-internal leak that `repo:publish-metadata` Check 2b exists to catch on the Cargo side —
because this crate declares no `include` allowlist (§7.1).

**Consequence for the umbrella design.** "Wheels only, no sdist" is retired and replaced by
"wheels **plus a verified sdist**". Three places record the false premise and must be corrected
as part of this work (§10).

### 2.1 What the wheel already gets right

Measured from the same probe — no work needed on any of these:

- Tagged `cp312-abi3`. `rs/Cargo.toml:102` already enables `pyo3`'s `abi3-py312`, so **one
  wheel per (OS, arch) covers CPython 3.12+**. The matrix does *not* multiply by Python
  version. This is the single largest divergence from a conventional cibuildwheel matrix and
  the reason this issue is smaller than SMA-428 despite covering the same platforms.
- Ships `paigasus_py_bindings/py.typed` and `paigasus_py_bindings/__init__.pyi` — maturin
  promotes the crate-root `.pyi` into the package. PEP 561 is satisfied; no stub work.

## 3. Scope

**In:** the wheel matrix; the sdist and its verification; PyPI packaging metadata for
`paigasus-py-bindings`; a Python arm on `repo:publish-metadata`; the gated `release` job
skeleton with the PyPI publish path; the re-founded release guard in `ci/actionlint/run.sh`;
SMA-556's two stub packages.

**Out, each with a reason:**

| Deferred | Reason |
| --- | --- |
| The `release` job's **npm** path, the napi↔release-plz tagging boundary, `@paigasus/wasm` packaging | SMA-579 owns them. Umbrella §7 says the tagging boundary must be settled before that path is written; it is still unresolved (umbrella §14 Q4). |
| Actually publishing anything | SMA-580 flips `PAIGASUS_RELEASE_ENABLED`. Everything here ships inert. |
| `win_arm64` wheels | No napi precedent, no runner in `prebuild.yml`. The verified sdist (§6) is the fallback. |
| SMA-535, SMA-560, SMA-434, SMA-552 | §11. |

## 4. Decisions

| # | Decision | Rationale |
| --- | --- | --- |
| D1 | Gated publish path, not build-only | Umbrella §12 assigns SMA-578 "§7's PyPI half"; SMA-579 owns "the release job's **npm** path", so the PyPI path is this issue's. Keeps SMA-580 to "flip the variable". |
| D2 | Six platforms, mirroring `prebuild.yml` | One kernel bound to three languages (ADR-0005). Asymmetric platform support between the napi and PyO3 faces is a footgun, and abi3 makes each platform *one* wheel. |
| D3 | Wheels **plus** a verified sdist | §2. The prohibition's premise is measured false, and the sdist is the only install path for the platforms the six legs miss. |
| D4 | Extend `repo:publish-metadata`, don't add a gate | It already sits in `ci.yml`'s `T=(…)` with negative-control and self-test scaffolding; a new gate pays the full SMA-541/553/530 tax for the same job. Its name is ecosystem-neutral. |
| D5 | A **reusable** `wheels.yml` (`on: workflow_call`) | One matrix definition, two consumers. PR-time verification exercises the exact job the release path runs. |
| D6 | OIDC trusted publishing | Umbrella §7 prefers it; no long-lived, exfiltratable credential exists. |
| D7 | Fold SMA-556 | Same work class and same files (§7.2). |

## 5. `wheels.yml` — the reusable build workflow

`.github/workflows/wheels.yml`. Triggers, all written as **block sequences** (`repo:actionlint`
fails all four keys loudly on inline flow):

- `workflow_call` — how `release.yml` consumes it.
- `pull_request` on `main`, filtered to the narrow inputs that can break a wheel build:
  the workflow itself, `.prototools`, `.moon/**`,
  `rs/crates/bindings/paigasus-py-bindings/**`, `rs/crates/libs/paigasus-kernel/**`,
  `rs/Cargo.{lock,toml}`, `rs/rust-toolchain.toml`, `py/packages/paigasus-kernel/**`.
- `push` to `main`, filtered to `rs/**` — post-merge verification, mirroring `prebuild.yml`'s
  split (that workflow's own comments explain why `rs/**` is too broad for a PR trigger: it
  would put a macOS job on most PRs in the repo).
- `workflow_dispatch`.

Not a required check, matching `prebuild.yml` — the `Protect main` ruleset requires only
`moon ci`, so a skipped run cannot wedge a merge.

### 5.1 The matrix

Six jobs, seven wheels, every one `cp312-abi3`:

| Leg | Runner | Target(s) | Expected platform tag |
| --- | --- | --- | --- |
| darwin | `macos-latest` | `aarch64-apple-darwin` **+** `x86_64-apple-darwin` in one job | `macosx_11_0_arm64`, `macosx_10_12_x86_64` |
| win-x64 | `windows-latest` | `x86_64-pc-windows-msvc` | `win_amd64` |
| linux-x64-gnu | `ubuntu-latest` | `x86_64-unknown-linux-gnu` (`--zig`) | `manylinux_2_17_x86_64` |
| linux-arm64-gnu | `ubuntu-latest` | `aarch64-unknown-linux-gnu` (`--zig`) | `manylinux_2_17_aarch64` |
| linux-x64-musl | `ubuntu-latest` | `x86_64-unknown-linux-musl` (`--zig`) | `musllinux_1_2_x86_64` |
| linux-arm64-musl | `ubuntu-latest` | `aarch64-unknown-linux-musl` (`--zig`) | `musllinux_1_2_aarch64` |

Both apple triples build in one `macos-latest` job for the reason `prebuild.yml` records: the
macOS SDK ships both slices, and merging drops a duplicated toolchain setup.

**`--zig` on all four linux legs, not only musl — the one deliberate divergence from
`prebuild.yml`.** There, zig supplied musl libc and the gnu legs built natively. Here that
would be wrong: `ubuntu-latest` ships glibc 2.39, so a native build tags `manylinux_2_39`, a
wheel almost no consumer can install. zig retargets glibc to 2.17 (manylinux2014). A welcome
side effect — no `ubuntu-24.04-arm` runners are needed at all, so this matrix is *cheaper* than
prebuild's despite covering the same platforms.

`maturin` drives `cargo-zigbuild` internally via its own `--zig` flag, so the setup mirrors
`prebuild.yml`'s existing `pip3 install ziglang` + `cargo install cargo-zigbuild` step.

### 5.2 Verification per leg

**Tag assertions are exact-equality, never substring.** This is `prebuild.yml`'s `lipo -archs`
lesson transplanted: a `grep -q x86_64` passes for a universal binary, i.e. it is vacuously
green in precisely the case worth catching. Each leg asserts its produced wheel filename's
platform tag equals the expected string.

Runtime verification reaches further than the napi matrix could, because three of the seven
wheels are executable on the runner that built them:

| Wheel | Verification |
| --- | --- |
| `macosx_11_0_arm64` | install into a clean venv on `macos-latest`, import, call across the FFI boundary |
| `win_amd64` | same, on `windows-latest` |
| `manylinux_2_17_x86_64` | same, on `ubuntu-latest` |
| `macosx_10_12_x86_64` | `lipo -archs` exact-equality on the `.so` inside the wheel |
| the three cross-built linux wheels | filename tag assertion + `file`/readelf-class inspection of the `.so` |

## 6. The sdist and the pure-Python face

A seventh, platform-independent job:

1. `maturin sdist` → `paigasus_py_bindings-<v>.tar.gz`.
2. Assert the sdist does **not** contain `moon.yml` (§7.1 makes this true; without the
   assertion the `include` allowlist could silently regress).
3. In a clean venv with a Rust toolchain: `pip install --no-binary :all: <sdist>`, then import
   and call. **An unverified sdist is worse than no sdist** — pip only falls back to it when no
   wheel matches, i.e. exactly when the consumer has no alternative, and a broken one fails
   mid-install with a cargo error.
4. `uv build` the pure-Python face `py/packages/paigasus-kernel` (one wheel + one sdist).

## 7. Packaging metadata

### 7.1 `paigasus-py-bindings`

`pyproject.toml` gains `description`, `readme`, `license`, `license-files`, `authors` and
`classifiers`, following `py/packages/paigasus-proto/pyproject.toml:4-17`. The crate dir gains
a real `LICENSE` (Apache-2.0, matching the repo root) and a `README.md`.

Two constraints:

- **`[project]` must remain the first table.** `ci/version-lockstep/run.sh`'s `write_site`
  substitutes the version field in place and relies on it (`run.sh:434-439`). Metadata is
  appended within `[project]`, never in front of it.
- **The SPDX-vs-classifier rule (SMA-378).** An SPDX `license` expression means the
  `License ::` trove classifier is **omitted**, not supplied alongside — PyPI hard-rejects the
  combination.

`Cargo.toml` gains an `include` allowlist so the sdist stops shipping `moon.yml`. Note this
crate has `publish = false`, so it is *not* in `EXPECTED_PUBLISHABLE` and Cargo-side Checks
1d/2b/2c do not reach it — which is exactly why the leak survived. The Python arm (§8) is what
covers it.

### 7.2 Folded SMA-556

`py/packages/paigasus-ml` and `py/packages/paigasus-workflows` each declare `README.md` and
`LICENSE` among their inherited `build` inputs (`.moon/tasks/python-project.yml:27`) and
**neither file exists**. Both build with `uv_build`, which does not auto-glob license files
(SMA-378), so a published wheel would carry no license text.

Each gains a real `LICENSE` and `README.md`, and `license-files = ["LICENSE"]` in its
`pyproject.toml`, under the same SPDX rule. Both stay at `0.0.0` — this fixes the packaging
defect without making them publishable.

SMA-556's fourth acceptance criterion (`moon query projects` reports zero untracked
`inputFiles` across the `py` workspace) is the verification step and is carried over verbatim.

## 8. `repo:publish-metadata` grows a Python arm

**Discovery rule: a py distribution is PyPI-bound iff its `[project] version != "0.0.0"`.**
This mirrors ADR-0011 S4's dormant-until-real and needs no new marker. Verified: `paigasus-ml`
and `paigasus-workflows` are both at `0.0.0`; `paigasus-kernel`, `paigasus-py-bindings` and
`paigasus-proto` are at `0.1.0`.

| Check | Assertion | Mirrors |
| --- | --- | --- |
| **P0** | the discovered set **equals** `EXPECTED_PYPI_PUBLISHABLE=("paigasus-kernel" "paigasus-py-bindings" "paigasus-proto")` | Check 0 — the non-vacuity control. The set is discovered from the very field the gate protects, so a shrunken set must be a hard failure, not a green run over nothing. |
| **P1** | each carries `description`, `readme`, `license`, `license-files`, `authors`, `classifiers`, and obeys the SPDX-vs-classifier rule | Check 1 / 1b |
| **P2** | the `README.md` and `LICENSE` those fields name **exist on disk** | — (SMA-378: `uv_build` does not auto-glob) |
| **P3** | the **built** artifact ships README + LICENSE and **not** `moon.yml` | Check 2b/2c — behavioural, not spelling-based |

P3 builds artifacts (`uv build` ×2, `maturin sdist` ×1). The gate already runs
`cargo publish --dry-run` per publish group, so it is already a heavy task; the plan measures
the added wall-clock and records it, as SMA-530 did for the release-parity controls.

**Bookkeeping.** The task's `inputs` grow to cover the py `pyproject.toml`/`README`/`LICENSE`
paths; `repo:input-liveness` then holds those globs live (SMA-553), so the entries must be
paths that exist after §7. The existing `--negative-control` gains py fixtures — at minimum a
dropped `license-files` and a deleted `LICENSE` — each staged into its own pristine scratch
tree, proving the arm can still report red. No new `T=(…)` entry and no CLAUDE.md marker edit
is needed: `repo:publish-metadata` is already in both.

## 9. `release.yml` — the gated skeleton

`release.yml` today has one live job, `release-pr`. This adds the second half of release-plz's
two-job pattern:

```
release        (if: vars.PAIGASUS_RELEASE_ENABLED == 'true')
  └─ release-plz release   → crates.io publish + git tags
wheels         (needs: release, uses: ./.github/workflows/wheels.yml)
publish-pypi   (needs: [release, wheels], permissions: id-token: write)
```

- **Publish order is `paigasus-py-bindings` first, then `paigasus-kernel`.** The face pins
  `==`, so the reverse order leaves it uninstallable in the window between the two uploads —
  the same ordering lesson as `paigasus-proto-derive` → `paigasus-proto` (umbrella §3).
- **Conditioned on an actual release.** `release-plz release --output json` reports what it
  released; the PyPI jobs run only when the kernel family moved, rather than on every push to
  `main`. PyPI would reject a duplicate version anyway, but a 400 is not a design.
- **Credentials live only here.** `release.yml` has no `pull_request` trigger (umbrella §7,
  review M2). OIDC means there is no token to exfiltrate even so.
- Nothing publishes until SMA-580 flips the variable.

### 9.1 The re-founded release guard, and its guard-the-guard obligations

Because this issue creates the `release` job, it also lands the assertion umbrella §9 specifies
and SMA-576 could not (§1). In `ci/actionlint/run.sh`: a **new verdict function** asserting the
`release` job's `if:` guard is present, references `PAIGASUS_RELEASE_ENABLED`, and is not
defeated by a `continue-on-error:` value other than the literal `false` or by a discarded exit
status.

Per the repo's own doctrine (`ci_targets.py:225-323`, *"That script cannot assert its own
invocation"*) this is a new verdict function against a new file, **not** an extension of check
8, whose scanning is keyed on `ci.yml`. It therefore requires all three of:

1. a new self-test table driving the verdict function through pass and fail fixtures;
2. `SELF_TEST_COUNT` **9 → 10** — check 9 asserts self-test *invocations* **and**
   *definitions*, so both halves move together;
3. a whole-line `ACTIONLINT_SH_CALL_SITES` entry in `ci/affected-graph/ci_targets.py` pinning
   the new production call site — because a fixture table exercises the verdict function, never
   its invocation, so deleting the production block would otherwise pass green
   (the SMA-542 lesson).

The guard protects the **mechanism**, not the **decision**: the variable remains flippable in
the UI with no PR. Umbrella §9's review M12 already accepts that trade explicitly.

## 10. Corrections to the umbrella design

§2's measurement falsifies a premise recorded in three places. All three are edited by this
work, so the repo does not keep asserting something known to be untrue:

1. `rs/crates/bindings/paigasus-py-bindings/pyproject.toml` — the `NOTE (publish deferred)`
   comment.
2. `docs/superpowers/specs/2026-08-22-sma-407-release-activation-design.md` §7, *The PyPI wheel
   problem (review M3)*.
3. …and its §15 risk row *"PyPI package uninstallable off linux/x86_64"*.

Each is amended to state what was measured, with the control, rather than silently deleted —
the claim was load-bearing for a decision, so its reversal is worth recording.

## 11. Folds considered and declined

| Ticket | Why not |
| --- | --- |
| **SMA-535** `py:typecheck` does not propagate from Rust | A Moon affected-graph scheduling problem; the fix re-baselines `ci/affected-graph`'s strict-equality cases. Different subsystem, and it would double this PR's CI-bookkeeping surface. |
| **SMA-560** the wrappers' Rust-source inputs are unasserted | A different gate with its own full bookkeeping tax. Adjacent in subject, not in mechanism. |
| **SMA-434** CI drift check for committed FFI glue | napi + wasm; nothing on the PyPI path. |
| **SMA-552** `--locked` unenforceable across the Moon graph | Touched only incidentally (the sdist embeds `Cargo.lock`). |
| **SMA-379** remove the pytest no-tests shim | Unrelated. |

## 12. Testing

| What | How |
| --- | --- |
| Every leg produces the right wheel | Exact-equality platform-tag assertion per leg (§5.2) |
| The wheels actually load | Native import-and-call on darwin-arm64, win_amd64, linux-x64-gnu; `lipo -archs` on darwin-x64 |
| The sdist is a real fallback | `pip install --no-binary :all:` into a clean venv, then import and call |
| The sdist ships nothing internal | Assert `moon.yml` absent |
| The metadata arm can report red | `--negative-control` py fixtures, each in its own pristine scratch tree |
| The release guard can report red | The new self-test table, plus check 9's mutation battery once `SELF_TEST_COUNT` is 10 |
| SMA-556 is closed | `moon query projects` reports zero untracked `inputFiles` across `py` |
| Nothing else regressed | The full `moon ci` graph per CLAUDE.md's marker-delimited command, `--base origin/main --include-relations` |

## 13. Risks

| Risk | Mitigation |
| --- | --- |
| The expected platform-tag strings and maturin's `x86_64-apple-darwin` deployment-target default are **assumptions until CI runs** | The plan treats the first CI run as a *measurement*, then pins the measured strings. Do not hand-write the assertions and call them verified. |
| `--zig` changes the glibc floor silently if maturin's default moves | The tag assertion is exact-equality, so a moved floor reds rather than shipping an uninstallable wheel |
| OIDC has no publisher registered at first upload | A **pending publisher** for both distributions, bound to this repo and `release.yml`, becomes a line in SMA-580's pre-flight checklist |
| SMA-579 edits `release.yml` in parallel and conflicts | This issue creates the job; SMA-579 adds a path *inside* it. Sequence 578 before 579, as the umbrella's `576 → (577 ‖ 578 ‖ 579) → 580` ordering already permits |
| `gh workflow run wheels.yml` 404s | Known (CLAUDE.md): a workflow is not dispatchable until it is on `main`. The `pull_request` trigger covers the PR itself |
| P3's artifact builds slow the gate | Measure and record the delta, as SMA-530 did for the release-parity controls |

## 14. Open questions for the plan

1. Does maturin's `include` handling honour a Cargo `include` allowlist for the **sdist** file
   list, or does it need `[tool.maturin] include` as well? The sdist is produced from
   `cargo package --list` (visible in the probe output), which suggests Cargo's allowlist is
   sufficient — but it must be measured, not inferred.
2. What exactly does `maturin sdist` name and place `pyproject.toml` as, given the crate is not
   at the sdist root? Affects the P3 assertion's path matching.
3. Does `release-plz release --output json` report released packages in a shape the PyPI job
   can condition on, at the pinned `0.3.158`? Read the source at the pinned tag, as SMA-576 did
   for `release_pr`.
4. Should `wheels.yml` cache `rs/target` per triple the way `prebuild.yml` does? If so it needs
   its own literal key discriminator — `actions/cache` skips its post-job save on an exact
   primary-key hit, so reusing prebuild's key shape would mean cold rebuilds forever.
