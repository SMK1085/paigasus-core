# SMA-578 — Release activation C: maturin cross-platform wheel matrix for `paigasus-py-bindings`

**Status:** Draft, **adversarial review incorporated 2026-08-28 (B1–B4, M1–M16, N1–N8)**
**Date:** 2026-08-28
**Linear:** [SMA-578](https://linear.app/smaschek/issue/SMA-578/release-activation-c-maturin-cross-platform-wheel-matrix-for-paigasus)
— child of [SMA-407](https://linear.app/smaschek/issue/SMA-407), unblocked by SMA-576 (Done).
**Folds in [SMA-556](https://linear.app/smaschek/issue/SMA-556)** (§7.2).
**Branch:** `feature/sma-578-release-activation-c-maturin-cross-platform-wheel-matrix-for`
**Targets:** `main` (currently `3f23758`).
**Umbrella design:** `docs/superpowers/specs/2026-08-22-sma-407-release-activation-design.md`
— this document implements **§7's PyPI half** and **corrects its §7/M3** (§2).
**Implements here:** the artifacts and the gate. **§9 is written but deferred to SMA-579**
(review **M12**) — kept in this document rather than split off, mirroring how the umbrella
keeps §6/§7 as inputs to its own children.
**References:** ADR-0005; ADR-0011 (S3 the tool owns every tag, S4 dormant-until-real);
SMA-419; SMA-428 (the napi matrix — shape and precedent); SMA-378 (`uv_build` license-files);
SMA-520 (prebuild's trigger-cost decision); SMA-529/SMA-530 (negative controls);
SMA-541/SMA-553 (gate bookkeeping); SMA-542 (guard-the-guard); SMA-576; SMA-577.

---

## 1. Problem

The kernel family sits at the `0.1.0` floor and `repo:version-lockstep` pins
`py/packages/paigasus-kernel` (the Python face) `==` to `paigasus-py-bindings` (the
maturin/PyO3 wheel). Neither can be installed from PyPI today, and **no wheel matrix exists
anywhere in the repo**. `prebuild.yml`'s six-leg matrix builds the *napi addon*; there is no
cibuildwheel and no maturin-action.

A single-runner build yields one wheel. Under the umbrella's "wheels only, no sdist" answer
there is no fallback, so `pip install paigasus-kernel` fails outright on macOS, Windows and
linux-aarch64.

Two secondary gaps ride along:

- `rs/crates/bindings/paigasus-py-bindings/pyproject.toml` carries essentially no PyPI
  metadata and the crate dir has no `LICENSE` or `README.md`. **Measured:** the built wheel's
  `METADATA` is **88 bytes**. Nothing gates PyPI metadata the way `repo:publish-metadata` gates
  crates.io (umbrella §14 Q8).
- **The release guard is currently unfounded.** Umbrella §9 records that
  `repo:publish-metadata` Check 3 goes *vacuously satisfied* at the `0.1.0` floor
  (`ci/publish-metadata/run.sh:198-200` — the `stubs` list is empty, so the block is skipped),
  and that a replacement must move into `ci/actionlint/run.sh`. SMA-576 listed that in scope but
  **could not implement it**: there was no `release` job to guard. Verified on `3f23758` —
  `ci/actionlint/run.sh` contains no `PAIGASUS_RELEASE_ENABLED` assertion and `SELF_TEST_COUNT`
  is still 9. That remains open, and §9.1 specifies it for whoever creates the job.

## 2. The measurement that redirects the design

The `pyproject.toml` caveat, umbrella §7 **review M3**, and §15's risk row all rest on one
claim: *a published sdist would not carry `rs/.cargo/config.toml`, whose apple-darwin
`-undefined dynamic_lookup` flags the `extension-module` cdylib needs — so a macOS consumer
building from sdist fails.*

**The observation is true; the conclusion is false.** The decisive experiment is not an
attribution argument about which tool injects which flag (review **M1** correctly notes a
control that fixes CWD cannot bind a subprocess that may `chdir`). It is the **consumer path
itself**, run end to end on macOS 25.6.0 / maturin 1.9.6 / cargo 1.95:

1. `maturin sdist` produced `paigasus_py_bindings-0.1.0.tar.gz`.
2. It was extracted to `/private/tmp/.../consumer/`, where **no `.cargo/config.toml` exists
   anywhere on the upward path** — verified, including the absence of `~/.cargo/config.toml`.
3. `maturin build` in that directory: **exit 0**,
   `paigasus_py_bindings-0.1.0-cp312-abi3-macosx_11_0_arm64.whl`.

Because the artifact under test is the published one and the environment is the consumer's, the
mechanism is irrelevant to the conclusion. (For context only, not as the argument: a plain
`cargo build -p paigasus-py-bindings --manifest-path rs/Cargo.toml` from the repo root fails
with `ld: symbol(s) not found for architecture arm64`, undefined `__Py_IncRef` /
`__Py_NoneStruct` — which is exactly what `rs/.cargo/config.toml`'s own comment says that file
is for: linking "**WITHOUT maturin**".)

**This experiment is a snapshot, and §6 turns it into a standing guarantee.** It was run on one
maturin version, one host, one target, natively. It therefore does **not** cover the darwin
*cross* build (`--target x86_64-apple-darwin`), Windows, or any other maturin version — which is
why §6's verification runs on three platforms rather than one, and §5.3 pins maturin.

### 2.1 What the sdist contains

```
paigasus_py_bindings-0.1.0/crates/libs/paigasus-kernel/{Cargo.toml,LICENSE,README.md,src/…,tests/…}
paigasus_py_bindings-0.1.0/crates/bindings/paigasus-py-bindings/{Cargo.toml,moon.yml,paigasus_py_bindings.pyi,src/lib.rs}
paigasus_py_bindings-0.1.0/{Cargo.lock,Cargo.toml,pyproject.toml,PKG-INFO}
```

maturin **vendors the workspace path dependency** and ships `Cargo.lock`. Three consequences,
all requiring action:

- It ships **`moon.yml`** — the identical repo-internal leak `repo:publish-metadata` Check 2b
  catches on the Cargo side — because this crate declares no `include` allowlist (§7.1).
- **It ships the workspace `Cargo.toml` verbatim, `[workspace.lints.rust] warnings = "deny"`
  included** (`rs/Cargo.toml:241-242`), and the crate carries `[lints] workspace = true`
  (`…/paigasus-py-bindings/Cargo.toml:30-31`). See §7.3 — review **B2**, and the most consequential
  finding of the review.
- It carries **no `rust-toolchain.toml`**, so a consumer builds with whatever rustc they have,
  against an edition-2024 / `rust-version = "1.95"` crate (§7.4).

### 2.2 What the wheel already gets right

Measured, no work needed: tagged `cp312-abi3` (`rs/Cargo.toml:102` already enables `pyo3`'s
`abi3-py312`, so **one wheel per (OS, arch) covers CPython 3.12+** — the matrix does not
multiply by Python version, which is why this issue is smaller than SMA-428 at equal platform
coverage); and it ships `py.typed` + `__init__.pyi`, so PEP 561 is satisfied.

## 3. Scope

**In:** the wheel matrix and its verification; the sdist and its three-platform verification;
PyPI packaging metadata, the `include` allowlist and the lint-table fix for
`paigasus-py-bindings`; a Python arm on `repo:publish-metadata`; SMA-556's two stub packages.

**Out, each with a reason:**

| Deferred | Reason |
| --- | --- |
| **The gated `release` job and the PyPI publish path (§9)** | Review **M12**, verified: `rs/release-plz.toml:36-47` sets `release = true` on `paigasus-node-bindings` and `paigasus-wasm` and sets no `git_tag_name`. So the moment `release-plz release` exists it tags the npm-facing crates in the default `{package}-v{version}` format — **the very decision umbrella §7 says must be settled before the release job is written** (§14 Q4, still open) and which SMA-579 owns. Creating the job here would make that decision by default. §9 stays in this document as SMA-579's input spec. |
| The napi↔release-plz tagging boundary, `@paigasus/wasm` packaging | SMA-579. |
| Actually publishing anything | SMA-580 flips `PAIGASUS_RELEASE_ENABLED`. |
| `win_arm64` wheels | No napi precedent, no runner in `prebuild.yml`. The verified sdist is the fallback. |
| SMA-535, SMA-560, SMA-434, SMA-552, SMA-379 | §11. |

**On the reversal of the original D1.** The first draft placed the gated release job here, and
that was reviewed and approved before **M12** was known. The review establishes that the job
cannot be created without silently settling SMA-579's tagging boundary, and B3/B4/M8/M10/M11
add four more unresolved questions clustered on the same job. Deferring it also **restores the
umbrella's `576 → (577 ‖ 578 ‖ 579) → 580` parallelism**, which the first draft broke with a
"sequence 578 before 579" risk row. This is a scope *reduction*, so nothing is lost that is not
written down here.

## 4. Decisions

| # | Decision | Rationale |
| --- | --- | --- |
| D1 | **Artifacts and gate only; the release job defers to SMA-579** | §3. Reverses the first draft on review **M12**. |
| D2 | Six platforms, mirroring `prebuild.yml` | One kernel bound to three languages (ADR-0005); asymmetric platform support between the napi and PyO3 faces is a footgun, and abi3 makes each platform *one* wheel. |
| D3 | Wheels **plus** a verified sdist | §2. The prohibition's premise is measured false, and the sdist is the only install path for platforms the six legs miss. |
| D4 | Extend `repo:publish-metadata` with **spelling-level** checks only | It already sits in `ci.yml`'s `T=(…)` with negative-control scaffolding. Behavioural artifact assertions move to `wheels.yml` (review **M6**) — see §8. |
| D5 | A **reusable** `wheels.yml` (`on: workflow_call`) | One matrix definition; SMA-579's release job consumes the same job PR-time verification exercises. |
| D6 | `wheels.yml` may **never** declare `secrets:` or `id-token: write` | Review **M14**. It is `pull_request`-triggered, and same-repo PRs receive repository secrets — umbrella §7/M2's vulnerability exactly. Recorded as a decision and asserted (§8.1). |
| D7 | Fold SMA-556 | Same work class, same files (§7.2). |
| D8 | Pin maturin | Review **M2**. §5.3. |

## 5. `wheels.yml` — the reusable build workflow

### 5.1 Triggers

All written as **block sequences** (`repo:actionlint` fails all four keys loudly on inline
flow), and **no brace expansion** — review **N1**, verified: `ci/actionlint/run.sh:1043`'s
charset regex `^[A-Za-z0-9._/*-]+$` rejects `rs/Cargo.{lock,toml}` as `rejected-charset`, and
`ci/affected-graph/task_inputs.py` carries a dedicated self-test row for the same shape. Each
path is its own entry.

- `workflow_call` — how SMA-579's release job will consume it.
- `pull_request` on `main`, **narrow** — review **M13**. `prebuild.yml:19-25,37-41` documents at
  length why `rs/**` is deliberately absent from its PR trigger ("a macOS job on every one of
  them would raise the bill", SMA-520), and the first draft reversed that decision without
  acknowledging it. The PR filter carries only: the workflow file, `.prototools`, `.moon/**`,
  `rs/.cargo/config.toml`, the bindings' `pyproject.toml` and `Cargo.toml`, and
  `py/packages/paigasus-kernel/pyproject.toml`. It carries the same explanatory comment
  `prebuild.yml` does.
- `push` to `main`, filtered to `rs/**` — where the broad coverage lives.
- `workflow_dispatch`.

Not a required check, matching `prebuild.yml`.

### 5.2 The matrix

Six jobs, seven wheels, every one `cp312-abi3`:

| Leg | Runner | Target(s) | Expected tag |
| --- | --- | --- | --- |
| darwin | `macos-latest` | both apple triples in one job | `macosx_11_0_arm64`, `macosx_10_12_x86_64` |
| win-x64 | `windows-latest` | `x86_64-pc-windows-msvc` | `win_amd64` |
| linux-x64-gnu | `ubuntu-latest` | `x86_64-unknown-linux-gnu` (zig) | `manylinux_2_17_x86_64` (compressed set — §5.4) |
| linux-arm64-gnu | `ubuntu-24.04-arm` | `aarch64-unknown-linux-gnu` (zig) | `manylinux_2_17_aarch64` |
| linux-x64-musl | `ubuntu-latest` | `x86_64-unknown-linux-musl` (zig) | `musllinux_1_2_x86_64` |
| linux-arm64-musl | `ubuntu-latest` | `aarch64-unknown-linux-musl` (zig) | `musllinux_1_2_aarch64` |

Both apple triples build in one `macos-latest` job for `prebuild.yml`'s reason: the macOS SDK
ships both slices, and merging drops a duplicated toolchain setup.

**zig on the linux legs, including gnu — the deliberate divergence from `prebuild.yml`.** There,
zig supplied musl libc and the gnu legs built natively. Here that would be wrong: `ubuntu-latest`
ships glibc 2.39, so a native build tags `manylinux_2_39`, which almost no consumer can install.

Three specifics the first draft glossed (review **M3**):

- The glibc floor comes from a **triple suffix** (`x86_64-unknown-linux-gnu.2.17`), not from a
  bare `--zig` flag.
- `--compatibility manylinux2014` / `--compatibility musllinux_1_2` is passed **explicitly**, so
  maturin's built-in auditwheel **errors** rather than silently emitting a `linux_*` tag PyPI
  rejects.
- musl targets default to `crt-static`, which a `crate-type = ["cdylib"]` cannot use;
  `-C target-feature=-crt-static` is set on the musl legs.

`ubuntu-24.04-arm` is retained for the gnu-arm64 leg (review **N7**) so that **one aarch64 wheel
is actually executed**, not merely inspected — the leg family with otherwise the weakest
verification.

### 5.3 maturin is pinned

Review **M2**. `.prototools` pins ten CLIs and `release.yml:71-78` states the doctrine
explicitly. maturin joins them (proto plugin, per the SMA-375 pattern), and
`pyproject.toml`'s `[build-system] requires` moves from `maturin>=1.7,<2` to the **measured
floor** `maturin>=1.9.6,<2`, with a comment recording why: an sdist consumer resolving maturin
1.7 would be building on a version §2's behaviour was never measured against.

### 5.4 Verification per leg

**Exact-equality, never substring** — `prebuild.yml`'s `lipo -archs` lesson (a `grep -q x86_64`
passes for a universal binary, i.e. is vacuously green in precisely the case worth catching).

Two refinements the first draft got wrong:

- **The platform tag is a compressed *set*** (review **M5**): maturin emits
  `…-cp312-abi3-manylinux_2_17_x86_64.manylinux2014_x86_64.whl`. "Equals the expected string"
  has no single answer for a `.`-joined set. The assertion splits on `.` and compares the
  resulting **set** to an expected set. Defining this *before* writing the assertion is what
  stops an implementer hitting a red on a correct wheel and "fixing" it by loosening to a
  substring — reintroducing the very vacuity this rule exists to prevent.
- **A tag is not a binary** (reviews **M3**, **M4**). Tags are derived from
  `MACOSX_DEPLOYMENT_TARGET` / sysconfig / the requested compatibility, not from the artifact,
  so tag assertions alone cannot detect a wheel that installs and then fails at import.

| Wheel | Verification |
| --- | --- |
| `macosx_11_0_arm64` | install into a clean venv on `macos-latest`, import, call across the FFI boundary |
| `win_amd64` | same, on `windows-latest` |
| `manylinux_2_17_x86_64` | same, on `ubuntu-latest` |
| `manylinux_2_17_aarch64` | same, on `ubuntu-24.04-arm` |
| `macosx_10_12_x86_64` | `lipo -archs` exact-equality **plus** `prebuild.yml:166-180`'s `otool -l` minimum-macOS assertion ported verbatim, asserting **tag-vs-binary agreement** — a cross-built x64 slice can otherwise inherit the host SDK's floor and silently drop 10.13–10.15 users |
| the cross-built musl wheels | tag-set assertion **plus** a max-symbol-version check (`objdump -T \| grep GLIBC_ \| sort -V \| tail -1`, or `auditwheel show`) — an ELF-*class* check reports only the machine type, so a wheel tagged `_2_17` whose `.so` needs `GLIBC_2.34` would pass it, install cleanly, and fail at import |

**Wheel `METADATA` is asserted too** (review **M16**). §1's motivating defect — 88 bytes — was
measured on the **wheel**, and maturin derives wheel `METADATA` from `[project]` through a
different code path than the sdist's `PKG-INFO`. At least one leg asserts `License`,
`Description-Content-Type`, `Classifier` and `.dist-info/licenses/LICENSE` are present.

## 6. The sdist, verified on three platforms

Review **B1**: the first draft made this "a seventh, **platform-independent** job", so the
verification would never have run on the platform §2's reversal is *about*. On Linux the
`-undefined dynamic_lookup` question does not arise, making the CI proof vacuous with respect to
the claim it exists to protect — and PyPI versions cannot be reused when a user reports the
regression.

The sdist job is therefore a **three-platform matrix** (`ubuntu-latest`, `macos-latest`,
`windows-latest`). On each:

1. `maturin sdist` (built once, on ubuntu, and shared as an artifact — the sdist is
   platform-independent even though its *verification* is not).
2. Assert `moon.yml` is absent (§7.1 makes this true; without the assertion the `include`
   allowlist can silently regress).
3. `pip install <sdist-path>` into a clean venv, then import and call. **Not
   `--no-binary :all:`** (review **N4**) — that forces source builds of *build* dependencies
   including maturin itself; installing a local sdist never consults a wheel for that package
   anyway.
4. The **macOS leg is explicitly labelled the standing control for §2**, with a comment saying so.

The **MSRV leg** (review **N6**): because the sdist carries no `rust-toolchain.toml`, one leg
runs against rustc at the declared `rust-version = "1.95"` floor rather than the repo's pinned
toolchain — otherwise the check proves nothing about the MSRV the crate advertises.

A separate platform-independent job runs `uv build` on `py/packages/paigasus-kernel`.

## 7. Packaging

### 7.1 `paigasus-py-bindings` metadata and `include`

`pyproject.toml` gains `description`, `readme`, `license`, `license-files`, `authors` and
`classifiers`, following `py/packages/paigasus-proto/pyproject.toml:4-17`. The crate dir gains a
real `LICENSE` (Apache-2.0) and `README.md` — the README also documenting the MSRV (§7.4).

**Correction to the first draft** (review **N2**): it claimed `[project]` must remain the first
table, citing `ci/version-lockstep/run.sh:434-439`. That is wrong — those lines are the *comment
describing a defect already fixed* by SMA-576 review finding 3. The live constraint is
`find_project_version_match` (`run.sh:443-471`): a `^\[project\]$` header line must exist, with a
`version = "..."` line before the next line beginning with `[`. Metadata added inside `[project]`
satisfies this regardless of table order.

`Cargo.toml` gains an `include` allowlist so the sdist stops shipping `moon.yml`. Note the crate
is `publish = false`, so Cargo-side Checks 1d/2b/2c do not reach it — which is exactly why the
leak survived — and §6 step 2, not the gate, is what holds it (review **M6**).

The SPDX rule (SMA-378): an SPDX `license` expression means the `License ::` trove classifier is
**omitted**, not supplied alongside; PyPI hard-rejects the combination.

### 7.2 Folded SMA-556

`py/packages/paigasus-ml` and `py/packages/paigasus-workflows` each declare `README.md` and
`LICENSE` among their inherited `build` inputs (`.moon/tasks/python-project.yml:27`) and neither
file exists. Both build with `uv_build`, which does not auto-glob license files (SMA-378).

Each gains a real `LICENSE`, a `README.md`, and `license-files = ["LICENSE"]`, under the same
SPDX rule. Both stay at `0.0.0`, and each additionally gains the **`Private :: Do Not Upload`**
trove classifier (review **M7**) — the ecosystem-standard marker, and the only mechanism that
makes **PyPI itself** refuse an accidental upload. No gate can do that.

SMA-556's fourth acceptance criterion (`moon query projects` reports zero untracked `inputFiles`
across the `py` workspace) is carried over verbatim as the verification step.

### 7.3 The lint table — review **B2**

`paigasus-py-bindings/Cargo.toml:30-31` is `[lints] workspace = true`, and `rs/Cargo.toml:241-242`
is `[workspace.lints.rust] warnings = "deny"`. **Verified by extracting the sdist: it ships the
workspace `Cargo.toml` verbatim, that table included.** So every sdist consumer compiles this
crate as a workspace/root package, where `--cap-lints allow` does **not** apply.

This is byte-for-byte the hazard `ci/publish-metadata/run.sh`'s Check 1c exists to prevent, and
which CLAUDE.md records as a standing rule. `paigasus-kernel` was already hardened for it
(`[lints.rust] warnings = "warn"`, with a comment about docs.rs); the bindings crate was not,
because nobody expected a third party to compile it. Making the sdist a supported install path
(D3) is exactly what changes that.

**Fix:** `paigasus-py-bindings` declares its own `[lints.rust] warnings = "warn"` (plus
`[lints.clippy] all = "warn"`) instead of inheriting, mirroring `paigasus-kernel`. CI strictness
is unaffected — the Moon `lint` task passes `-D warnings` explicitly.

**And the rule generalizes:** Check 1c is scoped to `publish = true` crates, which is why it
misses this one. §8's P1 extends the obligation to *any crate whose sources ship in a published
sdist*, which today means `paigasus-py-bindings` and the vendored `paigasus-kernel`.

### 7.4 MSRV

The sdist carries no `rust-toolchain.toml` and the crate is edition 2024 /
`rust-version = "1.95"`. A consumer on older rustc fails mid-`pip install` with a cargo error.
The MSRV is stated in the README, and §6's MSRV leg proves it.

## 8. `repo:publish-metadata` grows a Python arm

**Scope correction (review M6).** The first draft put artifact-building checks (`uv build` ×2,
`maturin sdist`) inside this gate. That gate is in `ci.yml:214`'s `T=(…)` — the required
`moon ci` check — with `toolchain: 'system'`; `ci.yml` installs no maturin and neither does
CLAUDE.md's worktree-provisioning sequence, so it would have put a new unpinned tool on the
critical path and made the gate unrunnable locally. **Behavioural artifact assertions live in
`wheels.yml`** (§5.4, §6), which already builds the artifacts and has maturin. This gate keeps
only spelling-level, pure-Python checks.

**Discovery (review M7).** The first draft's rule — `version != "0.0.0"` ⇒ PyPI-bound — is
unsound: in this repo that field means *"in a lockstep family"* (`repo:version-lockstep` writes
it), and `paigasus-py-bindings` is simultaneously `publish = false` on the Cargo side and
PyPI-bound. Instead:

- an **explicit marker**, `[tool.paigasus] pypi = true`, is the publish decision;
- the **scan set** is defined as `git ls-files`-tracked `pyproject.toml` under `py/packages/*/`
  plus the one bindings pyproject — not a filesystem `**/pyproject.toml` glob, which would sweep
  in `py/pyproject.toml` (a uv virtual root with **no `[project]` table**) and, in a provisioned
  tree, `ts/node_modules/.pnpm/…/node-gyp/gyp/pyproject.toml`;
- a missing `[project]`/`version` exits **2** (infrastructure), never 1 (the repo is wrong).

| Check | Assertion | Mirrors |
| --- | --- | --- |
| **P0** | the marked set **equals** `EXPECTED_PYPI_PUBLISHABLE`, strict equality | Check 0 — the non-vacuity control |
| **P1** | required `[project]` fields; the SPDX-vs-classifier rule; **and every crate whose sources ship in a published sdist declares its own non-denying `[lints.*]` table** (§7.3) | Check 1 / 1b / 1c |
| **P2** | the `README.md` and `LICENSE` those fields name **exist on disk** | — (SMA-378) |

`EXPECTED_PYPI_PUBLISHABLE` is `("paigasus-kernel" "paigasus-py-bindings")` — **not**
`paigasus-proto`, per §9.2.

**Bookkeeping.** The task's `inputs` grow to cover the py `pyproject.toml`/`README`/`LICENSE`
paths (one entry each, no brace expansion — **N1**); `repo:input-liveness` then holds those
globs live, so every entry must exist after §7. The existing `--negative-control` gains py
fixtures — at minimum a dropped `license-files`, a deleted `LICENSE`, and a crate re-inheriting
`warnings = "deny"` — each staged into its own pristine scratch tree. No `T=(…)` entry and no
CLAUDE.md marker edit is needed: `repo:publish-metadata` is already in both.

### 8.1 Asserting D6

`wheels.yml` carries a `pull_request` trigger, so a future refactor that moved the upload into it
— the natural move, since the artifacts are already there — would put `id-token: write` on a
PR-triggered workflow and reopen umbrella §7/M2. A check asserts `wheels.yml` declares neither
`secrets:` nor `id-token: write`, with D6's reasoning as its failure message.

## 9. Deferred to SMA-579 — the gated release job

Kept here as SMA-579's input spec (§3). Nothing in this section is implemented by SMA-578.

### 9.1 Shape

```
wheels        (uses: ./.github/workflows/wheels.yml)          ← everything reversible, first
release       (needs: wheels, if: vars.PAIGASUS_RELEASE_ENABLED == 'true')
                                → release-plz release: crates.io + tags
publish-pypi  (needs: [wheels, release])
```

**The ordering is inverted from the first draft, and that is the point** (review **B3**). The
draft ran `release → wheels → publish-pypi`, so `release-plz release` completed the crates.io
upload *and* cut the tags before a single wheel was built. A failure in the six-leg matrix — a
zig regression, a runner image change — would then leave crates.io permanently published, tags
permanently cut, and `paigasus-kernel` missing from PyPI while pinning `paigasus-py-bindings==X.Y.Z`.
Nothing forced that order: the release commit on `main` already carries the bumped versions, so
wheels can be built before release-plz runs. **Everything reversible goes before the first
irreversible step.** The workflow must carry this rationale as a comment, because the order looks
arbitrary otherwise.

Further requirements the review established:

- **Publish order within PyPI:** `paigasus-py-bindings` before `paigasus-kernel` — the face pins
  `==`, so the reverse leaves it uninstallable in the window between uploads (the derive→proto
  lesson, umbrella §3).
- **Idempotency (review M9):** the upload is two distributions; if the second fails, a retry
  re-uploads the first and PyPI returns 400 "file already exists", so the retry can never
  succeed unaided. Use `skip-existing: true` and make the job re-runnable. PyPI is
  delete-but-never-reuse; §10 carries the rollback row.
- **Version binding (review M10):** assert the built wheel's version equals the version
  release-plz reports for `paigasus-py-bindings` as a hard precondition of the upload.
- **crates.io credentials (review M11):** unspecified in the first draft, in the one job umbrella
  §7/M2 says must get credentials right. release-plz at the pinned `0.3.158` authenticates with
  `CARGO_REGISTRY_TOKEN`; crates.io trusted publishing needs an explicit OIDC→token exchange.
  SMA-579 must state which, and name the secrets/vars SMA-580's pre-flight creates.
- **PyPI credentials:** OIDC trusted publishing (`id-token: write`). The claim binds to the
  **calling** workflow filename, so the pending publisher registers against `release.yml`, not
  `wheels.yml`.
- **`concurrency:`** — `release.yml:13-15` currently holds `release-pr` at the *workflow* level
  with `cancel-in-progress: false`. Adding a multi-leg wheel matrix under the same group
  serializes every subsequent push to `main` behind it; the groups must be separated.
- **The tagging boundary** (**M12**) is settled here, not by default.

### 9.2 The re-founded release guard

Review **B4** rejected the first draft's rubric outright, and correctly. The draft transplanted
`assert_freshness_call_site`'s test — "the `if:` is present, not defeated by a
`continue-on-error:` other than literal `false`, exit status not discarded". That rubric guards
*a check that must be able to report red*. **This guard must prevent execution**, and its
bypasses are different:

- `publish-pypi` is gated only *transitively* through `needs:`. An added `if: always()` or
  `if: !cancelled()` un-gates the upload while the pinned `release` guard stays byte-identical
  and green.
- `continue-on-error: true` on `release` does not suppress a red — it makes a **failed** release
  job count as success for `needs:`, so a failed crates.io publish still lets wheels reach PyPI.
- The verdict function must find a **job-level** `if:` in a file already carrying seven
  step-level ones (`release.yml:45,63,77,81,85,106,125`), which a grep-shaped verdict cannot
  distinguish. `assert_freshness_call_site`'s whole-file `if:` ban is not transplantable for
  exactly that reason.

**The guard is therefore defined as:** *every job that can reach a registry is gated on
`PAIGASUS_RELEASE_ENABLED`, directly or through an unbroken `needs:` chain from a gated job, and
no such job carries `if: always()` / `if: !cancelled()`, or a `continue-on-error:` value other
than the literal `false`.* It parses YAML properly (Python, as `ci/publish-metadata`'s checks
already do), not bash grep. Both bypasses above become named fixture rows.

Guard-the-guard obligations, unchanged from the first draft except where noted:

1. a new self-test table driving the verdict function through pass and fail fixtures;
2. `SELF_TEST_COUNT` **9 → 10** — check 9 asserts invocations **and** definitions;
3. a whole-line `ACTIONLINT_SH_CALL_SITES` entry in `ci/affected-graph/ci_targets.py`. **It must
   sit at column 0** (review **N5**, verified at `ci_targets.py:390-409`): that haystack matches
   at column 0 deliberately, so a call site nested inside a function or `if` cannot satisfy it.
4. Check 9's mutation battery is derived from `run_self_tests`' body, so a tenth table adds a
   tenth concurrent mutant. `ci/actionlint/README.md` and `moon.yml` carry a measured cost table
   (~14.42s at 9) that must be **re-measured**, not adjusted by estimate.

The guard protects the **mechanism**, not the **decision** — umbrella §9's review M12 accepts
that trade explicitly.

### 9.3 `py/packages/paigasus-proto` — review **M8**

It is version-locked with the proto family and umbrella §10 reserves the name on PyPI, but no
publish path uploads it. Left unowned, every proto-family release burns a PyPI version that is
never uploaded, so the Python `paigasus-proto` permanently trails crates.io and can never be
published at a matching version — an irreversible skew introduced by omission.

It is therefore **excluded from `EXPECTED_PYPI_PUBLISHABLE`** in §8 (it carries no
`[tool.paigasus] pypi = true` marker yet), and **SMA-579 must either add it to the publish job or
file the issue that owns it**. Recording the choice is mandatory; making it silently is what
this finding forbids.

## 10. Corrections to the umbrella design

§2's experiment falsifies a premise recorded in three places. All three are amended — with the
measurement and its scope, not silently deleted, since the claim was load-bearing for a decision:

1. `rs/crates/bindings/paigasus-py-bindings/pyproject.toml` — the `NOTE (publish deferred)` comment.
2. the umbrella's §7 *The PyPI wheel problem (review M3)*.
3. its §15 risk row *"PyPI package uninstallable off linux/x86_64"*.

A fourth edit adds a **rollback row** to the umbrella's risk table (review **M9**): PyPI is
delete-but-never-reuse, so a partial upload is recovered by re-running with `skip-existing`, never
by re-cutting the version.

## 11. Folds considered and declined

| Ticket | Why not |
| --- | --- |
| **SMA-535** `py:typecheck` does not propagate from Rust | A Moon affected-graph scheduling problem; the fix re-baselines `ci/affected-graph`'s strict-equality cases. |
| **SMA-560** the wrappers' Rust-source inputs are unasserted | A different gate with its own bookkeeping tax. |
| **SMA-434** CI drift check for committed FFI glue | napi + wasm; nothing on the PyPI path. |
| **SMA-552** `--locked` unenforceable | Touched only incidentally (the sdist embeds `Cargo.lock`). |
| **SMA-379** remove the pytest no-tests shim | Unrelated. |

## 12. Testing

| What | How |
| --- | --- |
| Each leg produces the right wheel | Tag-**set** equality per leg (§5.4) |
| The wheels load | Native import-and-call on darwin-arm64, win_amd64, linux-x64-gnu, **linux-arm64-gnu** |
| The darwin x64 slice is right | `lipo -archs` + `otool -l` minimum-macOS, asserting tag-vs-binary agreement |
| The musl wheels are honestly tagged | Max GLIBC symbol-version check, not an ELF-class check |
| The wheels carry metadata | `METADATA` assertion on ≥1 leg (the defect was measured on the wheel) |
| The sdist is a real fallback | `pip install <sdist>` + import on **ubuntu, macOS and Windows**; macOS is §2's standing control |
| The sdist honours its MSRV | One leg at rustc 1.95, not the pinned toolchain |
| The sdist ships nothing internal | Assert `moon.yml` absent |
| The metadata arm can report red | `--negative-control` py fixtures, each in its own pristine scratch tree |
| SMA-556 is closed | `moon query projects` reports zero untracked `inputFiles` across `py` |
| Nothing else regressed | The full `moon ci` graph per CLAUDE.md's marker-delimited command, `--base origin/main --include-relations` |

**Acceptance evidence for the six build legs** (review **Q5**): `wheels.yml` is outside Moon, and
its narrowed `paths:` filter does not select it on most PRs. This PR touches
`.github/workflows/wheels.yml` and the bindings' `pyproject.toml`/`Cargo.toml`, all of which
**are** in the PR filter — so the matrix runs on this PR by construction. That is the evidence;
`moon ci` covers everything else.

## 13. Risks

| Risk | Mitigation |
| --- | --- |
| The expected tag **sets** and maturin's `x86_64-apple-darwin` deployment-target default are assumptions until CI runs | Treat the first run as a **measurement**, then pin what it measured. Do not hand-write the assertions and call them verified. |
| §2's conclusion regresses on a maturin/PyO3 change | §6's three-platform sdist verification, with the macOS leg labelled as the control; maturin pinned and the consumer floor raised to the measured version |
| A cross-built wheel is honestly tagged but unloadable | `--compatibility` makes auditwheel error; symbol-version and `otool` checks assert binary-vs-tag agreement |
| `cargo install --locked cargo-zigbuild` on four legs (prebuild does two), from source each time | Cache it, or record the added wall-clock (review **N6**) |
| A tenth self-test table slows `repo:actionlint` | Re-measure the cost table; do not estimate (review **N5**) |
| SMA-579 inherits §9 with its five unresolved questions | They are enumerated in §9.1/§9.2/§9.3 rather than discovered later |
| `gh workflow run wheels.yml` 404s until it is on `main` | Known (CLAUDE.md); the `pull_request` trigger covers this PR |

## 14. Open questions for the plan

1. **Does maturin honour Cargo's `include` for the sdist file list, or does it need
   `[tool.maturin] include`?** The probe shows the sdist is produced from `cargo package --list`,
   which suggests Cargo's allowlist suffices — but this is a **design fork, not a detail**
   (review **Q6**): if the answer is no, §7.1's mechanism and §6 step 2's assertion both change.
   **Measure before writing §7.1's implementation.**
2. With an `include` allowlist added, what asserts it stays correct as the crate dir gains files?
   Checks 1d/2c do not reach a `publish = false` crate (review **Q7**).
3. Does `release-plz release --output json` report released packages in a usable shape at the
   pinned `0.3.158`? Read the source at the pinned tag, as SMA-576 did for `release_pr`.
   *(SMA-579's question; recorded here because §9.1 depends on it.)*
4. Should `wheels.yml` cache `rs/target` per triple as `prebuild.yml` does? If so it needs its own
   literal key discriminator — `actions/cache` skips its post-job save on an exact primary-key
   hit, so reusing prebuild's key shape means cold rebuilds forever.
