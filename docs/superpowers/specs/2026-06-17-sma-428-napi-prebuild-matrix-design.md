# SMA-428 — napi-rs cross-platform `.node` prebuild matrix (infra-only, publish deferred)

> **Superseded in part by [SMA-520](https://linear.app/smaschek/issue/SMA-520) (2026-08-16).**
> The `darwin-x64 → macos-15-intel` mapping below (§1, and the run record at the end) is
> obsolete: that image was the last x86_64 macOS runner on Actions and retires in **August 2027**
> (§1's "Fall 2027" was the earlier, vaguer estimate; `actions/runner-images#13045` since fixed the
> window — the two dates below are not a contradiction, just different vintages).
> Both darwin targets now build in a single `macos-latest` (arm64) job — what §1 calls a
> *"fallback if `macos-15-intel` is constrained"* is now the design, and napi-rs's own generated
> CI does the same. The rest of this document is left as the historical record.

**Status:** approved design (brainstorm + staff review incorporated, ready for plan)
**Linear:** [SMA-428](https://linear.app/smaschek/issue/SMA-428/napi-rs-cross-platform-node-prebuild-matrix-npm-publish-for)
**Date:** 2026-06-17
**ADR:** ADR-0005 (kernel-once — pure Rust kernel bound to Py/Node/WASM), ADR-0006 (open-core boundary / publish discipline), ADR-0010/0011 (release tooling + strategy)
**Follow-up of:** [SMA-420](https://linear.app/smaschek/issue/SMA-420/stand-up-a-ts-kernel-binding-wasmnapi-wire-the-cascade-to-paigasus) — stood up the napi binding for a **single host** (macOS arm64) only; deferred the cross-platform prebuild matrix + npm publish here.
**Related:** SMA-407 (release activation — owns the actual publish), SMA-419 (the py-wheel sibling deferral), SMA-376 (kernel publish), SMA-434 (CI drift check for committed FFI glue).
**Reviewed by:** staff-engineer design review (`2026-06-17-sma-428-napi-prebuild-matrix-design-review.md`); dispositions in the final section.

## Goal

Build and **verify** a cross-platform `.node` prebuild pipeline for `@paigasus/node-bindings` —
the build matrix, per-platform packages, `optionalDependencies` wiring, and npm metadata —
**up to but not including `npm publish`**. Both `@paigasus/node-bindings` and `@paigasus/kernel`
stay `private: true` / `version: 0.0.0`. The `private:false` flip, the real version, the
kernel/proto lockstep, and the live release-plz workflow all remain with **SMA-407** (release
activation). This is the napi sibling of the deferred py-wheel publish (SMA-419 → SMA-407): land
and prove the machinery while it's dormant, so activation is a clean flip.

The single-host build that drives local `moon` build/test (SMA-420) is **untouched**; the matrix
is a separate, CI-only concern.

## Decisions resolved during brainstorming (+ staff review)

1. **Infra only; publish deferred (scope boundary vs SMA-407).** SMA-428 builds + verifies the
   prebuild/packaging pipeline but does **not** publish. Both packages stay `private: true` /
   `0.0.0`. The version flip (`0.0.0 → 0.1.0` floor), kernel/proto lockstep versioning, and
   turning on the dormant release-plz workflow are SMA-407's deliberate, risk-managed step
   (ADR-0011 S3 warns against hand-placing the first tag — the SMA-385 Helikon trap). Mirrors how
   SMA-398 landed dormant release config → SMA-407 activates it, and how SMA-419 deferred the
   py-wheel publish to SMA-407.
2. **Dedicated `prebuild.yml`, on `workflow_dispatch` + push-to-main, uploading artifacts.** A
   cross-platform `.node` matrix is inherently multi-OS, so it cannot live in the single
   `ubuntu-latest` `moon ci` job. It runs on manual dispatch (verify) and on push-to-main (catch
   breakage before activation), **not** on every PR — keeps PR CI fast on a placeholder kernel.
   The workflow is decoupled from `moon ci` and the affected-graph model (which is single-host).
3. **Verify = build matrix + dry-run assembly + a single-host real install-resolution check.**
   All 7 targets build and upload their `.node`; an `assemble` job generates the per-platform
   packages and runs `napi prepublish --dry-run --no-gh-release` + `npm pack --dry-run` to assert
   the publish artifact *structure* (os/cpu/libc, `main` paths, `optionalDependencies`), **and**
   does one *real* install on the CI host (`linux-x64-gnu`) to prove install-time platform
   resolution actually works for one target — not just by inspection (review H2). SMA-407 inherits
   a pipeline verified **up to, but not including, its riskiest links**: `npm publish`, and
   `napi prepublish` *without* `--dry-run` (the gh-release / tagging path — see §6).
4. **Generate `npm/<platform>/` in CI; commit nothing (review M1).** Per the napi v3 workflow,
   `napi create-npm-dirs` runs in CI; the per-platform `package.json`s and the main package's
   `optionalDependencies` are materialized ephemerally for the dry-run/assembly, **not** committed.
   This avoids committing generated artifacts whose only drift guard (SMA-434) is deferred, and
   dissolves the would-be 15-string `0.0.0` version lockstep — only the main `package.json`'s own
   `version: 0.0.0` stays committed. (Reverses the brainstorm's first instinct to commit
   reviewable scaffolds; the unguarded-generated-code smell decided it.)
5. **`@paigasus/node-bindings`-focused; `@paigasus/kernel` gets metadata only.** The matrix /
   packaging work is entirely a `@paigasus/node-bindings` concern (the host-coupled native
   package). `@paigasus/kernel` is pure TS glue whose `exports` point at **source** (`./src/*.ts`)
   with no `dist` build (tsup deferred by SMA-420) and `file:` deps on `@paigasus/node-bindings` +
   `@paigasus/wasm` — so it is **double-blocked** from real packaging (needs tsup/dist **and**
   version activation). SMA-428 only adds the static npm metadata it can have now, plus a
   breadcrumb comment. No tsup/dist work here.
6. **Native runners per target; musl cross-compiled via `cargo-zigbuild` (no container).** The five
   glibc/macOS/Windows targets build natively on their matching runners. The two musl targets were
   *first designed* to use the official napi-rs Alpine container, but CI proved that unworkable —
   GitHub forbids JS actions (checkout/cache/upload) in Alpine containers on **arm64** runners, and
   the image ships **pnpm 9** which can't read the repo's **pnpm-11** lockfile (see *CI verification
   findings* near the end). So musl builds on the glibc `ubuntu` runners via `napi build -x`
   (`cargo-zigbuild`; zig supplies the musl libc). zig is used **only** for Linux musl, never for
   Windows-MSVC or macOS, so its sharp edges don't apply.

## 1. Target matrix (7)

| napi platform     | Rust triple                  | Runner / method                                   |
| ----------------- | ---------------------------- | ------------------------------------------------- |
| `darwin-x64`      | `x86_64-apple-darwin`        | `macos-15-intel` (last native Intel; EOL Fall 2027) |
| `darwin-arm64`    | `aarch64-apple-darwin`       | `macos-latest`                                    |
| `win32-x64-msvc`  | `x86_64-pc-windows-msvc`     | `windows-latest`                                  |
| `linux-x64-gnu`   | `x86_64-unknown-linux-gnu`   | `ubuntu-latest`                                   |
| `linux-arm64-gnu` | `aarch64-unknown-linux-gnu`  | `ubuntu-24.04-arm`                                |
| `linux-x64-musl`  | `x86_64-unknown-linux-musl`  | `ubuntu-latest` + `cargo-zigbuild` (`napi build -x`)    |
| `linux-arm64-musl`| `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` + `cargo-zigbuild` (`napi build -x`) |

Every glibc/macOS/Windows leg builds natively on its arch; the two musl legs cross-compile via
`cargo-zigbuild` on the glibc `ubuntu` runners (no container — decision #6 + *CI verification
findings*).

**`macos-13` is retired** (GitHub brownout began Sept 2025, fully unsupported by 8 Dec 2025), so
`darwin-x64` uses **`macos-15-intel`** — GitHub's dedicated last-Intel image. Known clock:
**GitHub drops x86_64 macOS hosted runners entirely in Fall 2027** when the macOS-15 image
retires. So `darwin-x64` is a **time-boxed, sunset** matrix row, not a permanent one — and it is
**build-verified only** (the single-host install check in §2 runs on `linux-x64-gnu`; the Intel
addon is never *run* in CI; review L1). Revisit before real domain logic ships on Intel; the
fallback if `macos-15-intel` is constrained is to cross-build `x86_64-apple-darwin` on
`macos-latest` (Rust + Apple SDK cross-compile x86_64↔arm64 natively — **no zig**, so it does not
trip decision #6's zig concern), at the cost of the same never-run-on-Intel caveat.

## 2. New workflow — `.github/workflows/prebuild.yml`

Decoupled from `moon ci` (single-host, affected-graph-bound). Tooling is pinned via
`moonrepo/setup-toolchain` + `moon setup` so node/pnpm/rust match `.prototools` /
`rs/rust-toolchain.toml`; napi is invoked directly (not through Moon).

- **Triggers:** `workflow_dispatch` and `push: branches: [main]`.
- **Permissions:** `contents: read` only — no publish creds. The dry-run passes **`--no-gh-release`**
  (required: `ghRelease` defaults **on**, so without it `prepublish` enters `createGhRelease` and
  fails on the shallow CI checkout — see *CI verification findings*), so no GitHub release is created
  and no `contents: write` is needed. SMA-407 adds registry auth / `id-token` / release perms when it
  turns publish on.
- **`build` job** — `strategy.matrix` over the 7 targets `{ platform, target, runner, zig }`:
  1. checkout
  2. `moonrepo/setup-toolchain` + `moon setup` (pinned node/pnpm/rust) — every leg
  3. `rustup target add <triple>` against the pinned **1.95.0** toolchain (run from `rs/` so the
     `rust-toolchain.toml` override applies — verified pattern, mirrors `ci.yml`'s serial
     pre-install)
  4. musl legs only: set up zig + cargo-zigbuild (`pip install ziglang` + `cargo install cargo-zigbuild`)
  5. `pnpm --dir ts install --frozen-lockfile`
  6. `pnpm exec napi build --platform --release --target <triple>` (musl legs add `-x` for
     `cargo-zigbuild`) in `rs/crates/bindings/paigasus-node-bindings` (`--platform` emits the
     platform-suffixed `paigasus-node-bindings.<platform>.node` filename)
  7. upload `paigasus-node-bindings.<platform>.node` as a CI artifact
  - **All legs run on real glibc/macOS/Windows runners (no job-level container)**, so the GitHub JS
    actions and the pinned pnpm-11 work uniformly. The two **musl** legs cross-compile with
    `cargo-zigbuild` (step 4 + `-x`); this replaced the originally-designed Alpine container (review
    M2), which CI proved unworkable — see *CI verification findings*.
  - **Caching (review L2):** mirror `ci.yml`'s Rust cache (`~/.cargo` + `rs/target`) keyed on
    `runner.os` + **triple** + `rust-toolchain.toml` hash + `Cargo.lock` (cross-target artifacts
    differ by triple, so the triple must be in the key).
- **`assemble` job** (`needs: build`):
  1. download all build artifacts
  2. `napi create-npm-dirs` — generate the seven `npm/<platform>/` package dirs in CI (not
     committed; decision #4), then `napi artifacts --npm-dir npm` to sort each downloaded `.node`
     (from the `actions/download-artifact` default `./artifacts` dir) into its platform dir
  3. `napi prepublish --dry-run --no-gh-release --npm-dir npm` + `npm pack --dry-run` on the main +
     per-platform packages — assert os/cpu/libc, `main` paths, and `optionalDependencies` all
     resolve, and that the **main tarball ships loader-only** (no `.node`; this is the `files` fix in
     §3). `--dry-run` keeps it filesystem-/registry-inert; **`--no-gh-release` is required** (see §6 +
     *CI verification findings*), so no GitHub release is created.
  4. **Single-host install-resolution check (review H2).** On this `ubuntu-latest` host
     (= `linux-x64-gnu`, one of the seven targets): `npm pack` the main package + the
     `@paigasus/node-bindings-linux-x64-gnu` per-platform package, install them into a scratch
     project so the optional dep resolves *through the package path* (not the loader's local-`.node`
     fallback, which the §3 `files` fix removes from the main tarball), and assert
     `require('@paigasus/node-bindings').sum(2, 3) === 5`. Proves install-time platform resolution
     end-to-end for one real target at near-zero cost. (The exact local-resolution incantation —
     install both tarballs into the scratch project vs a throwaway local registry — is a spike
     check, §6.)
  - **No `npm publish` anywhere.** Both packages stay `private: true` / `0.0.0`.

## 3. `rs/crates/bindings/paigasus-node-bindings/package.json`

- Extend the `napi` block with `targets` = the 7 triples (the correct v3 schema key — v2's
  `napi.triples` was renamed) so `create-npm-dirs` / `prepublish` know the full set.
- Add npm metadata: `repository`, `homepage`, `keywords`, `description`, `engines.node`,
  `publishConfig.access: public`. Keep `private: true` / `version: 0.0.0`.
- **Fix `files`:** drop `*.node` from `files` (currently `["index.js", "index.d.ts", "*.node"]` →
  `["index.js", "index.d.ts"]`). In the optionalDependencies model the main package ships **only**
  the loader glue; the `.node` binaries ship in the per-platform packages. Leaving `*.node` in
  would wrongly bundle a locally-built host `.node` into the main tarball — the `npm pack --dry-run`
  (§2.3) asserts loader-only, and the §2.4 install check would otherwise load via the bundled
  fallback instead of exercising real resolution.
- **No committed `optionalDependencies`** and **no committed `npm/<platform>/` scaffolds**
  (decision #4) — both are generated in CI. At SMA-407, `napi version` / `napi prepublish` **owns**
  the version derivation + the `optionalDependencies` rewrite (not hand edits); SMA-428 commits
  only the single `version: 0.0.0` here.

## 4. `ts/packages/paigasus-kernel/package.json` (metadata only)

- Add static npm metadata: `repository`, `keywords`, `description`, `publishConfig`. Keep
  `private: true` / `version: 0.0.0`; **no** `exports` change, **no** tsup/dist.
- Extend the existing `_comment_exports` breadcrumb (or add a sibling `_comment`) noting publish is
  double-blocked: (a) `exports` point at source — needs tsup/dist (SMA-420 deferral), and
  (b) version activation lives in SMA-407.

## 5. `moon.yml` — unchanged

`rs/crates/bindings/paigasus-node-bindings/moon.yml` and `ts/packages/paigasus-kernel/moon.yml`
are **not** touched. The local single-host build/test chain (`paigasus-kernel-ts:build`/`:test`
running `napi build --platform` for the dev host) is unchanged, so local dev and the existing
`moon ci` are unaffected. The matrix is a separate CI-only workflow.

## 6. Release-tool boundary (`napi prepublish` vs release-plz) — SMA-407 hand-off (review M3)

On `@napi-rs/cli` 3.7.2, `napi prepublish`'s **`ghRelease` defaults ON** and `--tag-style` defaults
to **`lerna`**. `--no-gh-release` **does** turn it off (clipanion auto-negates booleans) even though
it isn't listed in `--help` — the Task-1 spike misread this; CI confirmed it (see *CI verification
findings*). Without `--no-gh-release`, `prepublish` enters `createGhRelease`→`getRepoInfo` and
**fails on the shallow CI checkout** ("No release commit found") even under `--dry-run`. So the
dry-run **passes `--no-gh-release`** → no GitHub release, no `contents: write` needed. When
**SMA-407 removes `--dry-run`** and opts into `--gh-release` / a real publish, napi's
**lerna-style tagging** (`@paigasus/node-bindings@vX.Y.Z`) must be reconciled with this repo's
**release-plz** machinery (its vendored proto plugin tags `release-plz-v*` / per-crate patterns,
SMA-398). Two release tools with two tagging schemes against one repo is a duplicate-tag /
double-publish incident waiting for activation day.

- **This issue:** pass **`--no-gh-release`** on the dry-run, so the workflow needs no write
  permission and creates no release.
- **Deliberately deferred to SMA-407 (not guessed here):** whether/how to use `--gh-release`, the
  `--tag-style` value, and the division of labor between `napi prepublish` and release-plz (who
  derives the version, who tags, who publishes to npm). These depend on the release-plz integration
  SMA-407 designs. Recorded as an explicit SMA-407 input, not a silent default.

## Primary risks → de-risk first (spike before the workflow)

1. **`@napi-rs/cli` v3 command + schema surface.** Confirm the exact v3 subcommands
   (`create-npm-dirs`, `artifacts --npm-dir npm`, `prepublish --dry-run --no-gh-release`) and the
   `napi.targets` package.json schema against the pinned `^3`. **(Done — Task-1 spike + CI: confirmed
   3.7.2; `artifacts` default input dir is `./artifacts`; `NAPI_RS_ENFORCE_VERSION_CHECK` must stay
   unset; `--no-gh-release` IS required and IS accepted despite being absent from `--help` — see *CI
   verification findings* + `2026-06-17-sma-428-spike-findings.md`.)**
2. **Single-host install-resolution mechanism (§2.4).** Confirm the exact way to make the
   `@paigasus/node-bindings-linux-x64-gnu` optional dep resolvable locally (install both tarballs
   into the scratch project vs a throwaway local registry) so the assertion loads via the **package
   path**, not the loader's local-`.node` fallback. Watch napi's `NAPI_RS_ENFORCE_VERSION_CHECK`
   (off by default) given the `0.0.0` placeholder version.
3. **musl cross-compile (§2 / decision #6).** *Obsolete:* the Alpine container was dropped after CI
   failures; musl now cross-compiles via `cargo-zigbuild` on the glibc runners with the pinned
   1.95.0 toolchain (see *CI verification findings*).
4. **`macos-15-intel` availability + cross-build fallback (§1 / H1, L1).** Confirm `macos-15-intel`
   runs the build; verify the `--target x86_64-apple-darwin` on `macos-latest` fallback compiles if
   needed.

## Verification (maps to acceptance criteria)

1. **Matrix build** — `prebuild.yml` dispatched: all 7 build legs green, each uploads its
   `paigasus-node-bindings.<platform>.node` artifact.
2. **Dry-run assembly** — the `assemble` job's `napi prepublish --dry-run --no-gh-release` +
   `npm pack --dry-run` succeed and show: a loader-only main package (no `.node`), exactly one
   `.node` per platform package, correct `os`/`cpu`/`libc`, and 7 `optionalDependencies`.
3. **Single-host install resolution** — the `linux-x64-gnu` install check resolves exactly the
   `linux-x64-gnu` optional dep and `require('@paigasus/node-bindings').sum(2, 3) === 5`.
4. **No publish / no state change** — no `npm publish` runs; no GitHub release is created; both
   packages remain `private: true` / `0.0.0`; nothing under `npm/` is committed.
5. **No regression** — existing `moon ci` stays green and unchanged; local
   `moon run paigasus-kernel-ts:build`/`:test` still work.

## Out of scope (deferred, with owners)

- **Actual publish** — `private: false`, version off `0.0.0`, kernel/proto lockstep versioning,
  the live release-plz workflow, and the `napi prepublish` ↔ release-plz boundary (§6: tag-style,
  who tags/publishes) → **SMA-407** (ADR-0011).
- **tsup/dist for `@paigasus/kernel`** — which also unblocks `@paigasus/kernel` + `@paigasus/wasm`
  publish → SMA-420 deferral / its own issue.
- **maturin py-wheel matrix** (manylinux/musllinux/macos/windows) — the py sibling of this work →
  SMA-419 / SMA-407.
- **CI drift check for committed FFI glue** (`index.js`) → **SMA-434**. (Note: decision #4 means
  SMA-428 commits **no** `npm/` artifacts, so the only committed generated glue remains `index.js`
  / `index.d.ts`; SMA-434's surface is unchanged by this issue.)
- **Full per-OS install/import smoke** (install + run on all 7 OSes) — considered and declined; the
  single-host check (§2.4) is the chosen fidelity level. `darwin-x64` in particular is
  build-verified only (review L1).
- **Real kernel domain logic** — `sum` remains a deliberate placeholder.

## Review dispositions (staff review, 2026-06-17)

- **H1 (High — `macos-13` retired, leg dead on arrival) — accepted, verified, design changed.**
  Confirmed via the GitHub changelog + `actions/runner-images#13046`: macos-13 brownout Sept 2025,
  fully unsupported 8 Dec 2025. §1 now uses **`macos-15-intel`** (not as a "fallback" but as the
  primary), records the **Fall-2027 x86_64-macOS EOL** as a known sunset, and notes the
  no-zig cross-build fallback. `darwin-x64` flagged build-verified-only.
- **H2 (High — dry-run never tests install resolution) — accepted, design changed.** Added the
  single-host (`linux-x64-gnu`) real install-resolution check (§2.4, AC #3) and softened the
  "verified pipeline" claim (decision #3) to name the links SMA-407 still must prove (publish,
  prepublish-without-dry-run / gh-release / tagging).
- **M1 (Medium — committing `npm/` runs against v3 guidance + unguarded until SMA-434) — accepted,
  design changed.** Switched to **generate `npm/` in CI, commit nothing** (decision #4, §2.2, §3).
  Removes the unguarded-generated-artifact smell and dissolves the version lockstep (M4).
- **M2 (Medium — musl legs use the image toolchain, not the proto pin) — accepted, then superseded
  by CI.** Originally resolved by accepting the Alpine image's Rust (leaf-artifact rationale). CI then
  proved the whole job-level Alpine container unworkable (arm64 JS-actions ban + pnpm-9 image), so the
  container was dropped entirely — musl now cross-compiles via `cargo-zigbuild` on the glibc runners
  with the **pinned** 1.95.0 toolchain, removing the M2 concern rather than mitigating it (see *CI
  verification findings*).
- **M3 (Medium — `prepublish` gh-release/lerna-tag defaults) — accepted; the reviewer + docs were
  right, CI confirmed.** `ghRelease` defaults ON and `--no-gh-release` turns it off (clipanion
  auto-negation, absent from `--help`). The Task-1 spike wrongly concluded `--no-gh-release` didn't
  exist; the first `assemble` run then failed (`createGhRelease`→`getRepoInfo`, "No release commit
  found" on the shallow checkout), and re-adding `--no-gh-release` fixed it (§2.3, §6). **Deviation
  from the reviewer's "pin `--tag-style` too":** left tag-style + the napi/release-plz division as an
  explicit SMA-407 decision (inert under dry-run).
- **M4 (Medium — 15 lockstep `0.0.0` strings) — accepted, dissolved by M1.** Generate-in-CI means
  only the main `package.json` `version` is committed; `napi version`/`prepublish` owns the bump at
  SMA-407 (§3).
- **L1 (Low — `darwin-x64` build-verified only) — accepted, noted** in §1 + Out of scope.
- **L2 (Low — caching unspecified) — accepted.** §2 adds the `ci.yml` Rust-cache pattern with the
  triple in the key.
- **L3 (Process — spec narrows the Linear ticket) — accepted.** SMA-428's Linear description
  updated to mark publish + install-resolution-publish as handed to SMA-407 so the ticket isn't
  closed with literal scope bullets unmet.

## CI verification findings (2026-06-18)

`prebuild.yml` was verified on real CI via a temporary branch trigger (reverted after green). Two
design points changed under contact; both are folded into the sections above:

1. **musl: Alpine container → `cargo-zigbuild` (decision #6 + M2).** The first run failed both musl
   legs. `linux-arm64-musl`: GitHub forbids JS actions (`actions/checkout`) in Alpine containers on
   arm64 runners ("JavaScript Actions in Alpine containers are only supported on x64 Linux runners").
   `linux-x64-musl`: the `nodejs-rust:lts-alpine` image ships Node 18 / **pnpm 9**, which can't read
   the repo's **pnpm-11** lockfile (`ERR_PNPM_LOCKFILE_CONFIG_MISMATCH`). Fix (user-approved): drop
   the job-level container; build both musl targets on the glibc `ubuntu` runners via `napi build -x`
   (`cargo-zigbuild`, zig via `pip install ziglang`). Both musl legs then passed.
2. **`prepublish` needs `--no-gh-release` (M3).** Omitting it, `prepublish` entered
   `createGhRelease`→`getRepoInfo` and failed on the shallow CI checkout ("No release commit found")
   even under `--dry-run` — `ghRelease` defaults **on**. `--no-gh-release` IS accepted (clipanion
   auto-negation) despite not appearing in `--help`; re-adding it fixed the `assemble` job.

**Final result (run on commit `976de97`):** all **7 build legs green** (darwin-x64 on `macos-15-intel`
— H1 validated; darwin-arm64; win32-x64-msvc; linux x64/arm64 gnu; linux x64/arm64 musl via zig)
**and** the `assemble` job green — `prepublish --dry-run --no-gh-release` clean, `npm pack` main package
loader-only (`main package is loader-only OK`), and the single-host `linux-x64-gnu` install-resolution
check loaded `sum(2,3)===5` (`install-resolution + FFI load OK`). No `npm publish`, no GitHub release
created, both packages still `private:true` / `0.0.0`.
