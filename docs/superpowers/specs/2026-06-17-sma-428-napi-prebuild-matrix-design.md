# SMA-428 — napi-rs cross-platform `.node` prebuild matrix (infra-only, publish deferred)

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
6. **Native runners per target + official napi-rs Alpine Docker images for musl.** Each target
   builds on its matching native-arch GitHub runner (GitHub's free `ubuntu-24.04-arm` removes the
   need to cross-compile arm64); only the two musl targets swap in the official `napi-rs` Alpine
   container. Canonical `@napi-rs/cli` scaffold shape — battle-tested, copy-adaptable — and avoids
   zig cross-compilation's sharp edges (Windows-MSVC, macOS SDK).

## 1. Target matrix (7)

| napi platform     | Rust triple                  | Runner / method                                   |
| ----------------- | ---------------------------- | ------------------------------------------------- |
| `darwin-x64`      | `x86_64-apple-darwin`        | `macos-15-intel` (last native Intel; EOL Fall 2027) |
| `darwin-arm64`    | `aarch64-apple-darwin`       | `macos-latest`                                    |
| `win32-x64-msvc`  | `x86_64-pc-windows-msvc`     | `windows-latest`                                  |
| `linux-x64-gnu`   | `x86_64-unknown-linux-gnu`   | `ubuntu-latest`                                   |
| `linux-arm64-gnu` | `aarch64-unknown-linux-gnu`  | `ubuntu-24.04-arm`                                |
| `linux-x64-musl`  | `x86_64-unknown-linux-musl`  | `ubuntu-latest` + napi-rs Alpine image            |
| `linux-arm64-musl`| `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` + napi-rs Alpine image         |

Every leg builds on its **native arch**; musl runs the build inside the official Alpine container.

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

Decoupled from `moon ci` (single-host, affected-graph-bound). Host tooling is pinned via
`moonrepo/setup-toolchain` + `proto install` so node/pnpm/rust match `.prototools` /
`rs/rust-toolchain.toml`; napi is invoked directly (not through Moon).

- **Triggers:** `workflow_dispatch` and `push: branches: [main]`.
- **Permissions:** `contents: read` only — no publish creds. The dry-run **omits `--gh-release`**
  (an opt-in presence flag — there is **no** `--no-gh-release` on `@napi-rs/cli` 3.7.2,
  spike-confirmed), so no GitHub release is created and no `contents: write` is needed. SMA-407 adds
  registry auth / `id-token` / release perms when it turns publish on.
- **`build` job** — `strategy.matrix` over the 7 targets `{ platform, target, runner, useContainer }`:
  1. checkout
  2. `moonrepo/setup-toolchain` + `proto install` (pinned node/pnpm) — *host* legs only
  3. `rustup target add <triple>` against the pinned **1.95.0** toolchain (run from `rs/` so the
     `rust-toolchain.toml` override applies — verified pattern, mirrors `ci.yml`'s serial
     pre-install)
  4. `pnpm --dir ts install --frozen-lockfile`
  5. `pnpm exec napi build --platform --release --target <triple>` in
     `rs/crates/bindings/paigasus-node-bindings` (`--platform` emits the platform-suffixed
     `paigasus-node-bindings.<platform>.node` filename)
  6. upload `paigasus-node-bindings.<platform>.node` as a CI artifact
  - **musl legs** run steps 3–5 **inside** the official napi-rs Alpine container, which ships its
    **own** Rust/Node — the host's `setup-toolchain`/`proto install` (step 2) do **not** reach into
    the container's filesystem/PATH. **Decision (review M2):** accept the **image's** toolchain for
    the two musl legs (the canonical napi-rs template does the same), rather than re-installing
    proto inside. Rationale: the `.node` is a **leaf** artifact — nothing links its rmeta across
    the npm boundary — so a musl leg built with the image's rustc carries none of the SMA-389
    cross-version `E0514` hazard. This is a *written, conscious* choice, not an accident of step
    ordering. (Confirm in the spike that the image's Rust is ≥ the kernel's MSRV.)
  - **Caching (review L2):** mirror `ci.yml`'s Rust cache (`~/.cargo` + `rs/target`) keyed on
    `runner.os` + **triple** + `rust-toolchain.toml` hash + `Cargo.lock` (cross-target artifacts
    differ by triple, so the triple must be in the key). Container-leg caching is best-effort
    (paths differ inside Alpine); accept colder musl builds if needed.
- **`assemble` job** (`needs: build`):
  1. download all build artifacts
  2. `napi create-npm-dirs` — generate the seven `npm/<platform>/` package dirs in CI (not
     committed; decision #4), then `napi artifacts --npm-dir npm` to sort each downloaded `.node`
     (from the `actions/download-artifact` default `./artifacts` dir) into its platform dir
  3. `napi prepublish --dry-run --npm-dir npm` + `npm pack --dry-run` on the main + per-platform
     packages — assert os/cpu/libc, `main` paths, and `optionalDependencies` all resolve, and that
     the **main tarball ships loader-only** (no `.node`; this is the `files` fix in §3). `--dry-run`
     keeps it filesystem-/registry-inert; **`--gh-release` is omitted** (opt-in presence flag — no
     `--no-gh-release` exists on 3.7.2, spike-confirmed), so no GitHub release is created (see §6).
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

On `@napi-rs/cli` 3.7.2 (Task-1 spike-confirmed), `napi prepublish`'s **`--gh-release` is an opt-in
presence flag** (there is **no** `--no-gh-release`), and `--tag-style` defaults to **`lerna`**. So
the dry-run here simply **omits `--gh-release`** → no GitHub release, no `contents: write` needed.
But **SMA-407 activates by removing `--dry-run`** and will need `--gh-release` / a real publish path
— at which point napi's **lerna-style tagging** (`@paigasus/node-bindings@vX.Y.Z`) must be
reconciled with this repo's **release-plz** machinery (its vendored proto plugin tags
`release-plz-v*` / per-crate patterns, SMA-398). Two release tools with two tagging schemes against
one repo is a duplicate-tag / double-publish incident waiting for activation day.

- **This issue:** **omit `--gh-release`** on the dry-run (the safe default — nothing to neutralize),
  so the workflow needs no write permission and creates no release.
- **Deliberately deferred to SMA-407 (not guessed here):** whether/how to use `--gh-release`, the
  `--tag-style` value, and the division of labor between `napi prepublish` and release-plz (who
  derives the version, who tags, who publishes to npm). These depend on the release-plz integration
  SMA-407 designs. Recorded as an explicit SMA-407 input, not a silent default.

## Primary risks → de-risk first (spike before the workflow)

1. **`@napi-rs/cli` v3 command + schema surface.** Confirm the exact v3 subcommands
   (`create-npm-dirs`, `artifacts --npm-dir npm`, `prepublish --dry-run`) and the `napi.targets`
   package.json schema against the pinned `^3`. **(Done — Task-1 spike: confirmed 3.7.2; no
   `--no-gh-release` — `--gh-release` is opt-in; `artifacts` default input dir is `./artifacts`;
   `NAPI_RS_ENFORCE_VERSION_CHECK` must stay unset. See `2026-06-17-sma-428-spike-findings.md`.)**
2. **Single-host install-resolution mechanism (§2.4).** Confirm the exact way to make the
   `@paigasus/node-bindings-linux-x64-gnu` optional dep resolvable locally (install both tarballs
   into the scratch project vs a throwaway local registry) so the assertion loads via the **package
   path**, not the loader's local-`.node` fallback. Watch napi's `NAPI_RS_ENFORCE_VERSION_CHECK`
   (off by default) given the `0.0.0` placeholder version.
3. **musl image toolchain (§2 / M2).** Confirm the napi-rs Alpine image's bundled Rust is ≥ the
   kernel MSRV (1.95.0) so the accepted-image-toolchain decision is sound.
4. **`macos-15-intel` availability + cross-build fallback (§1 / H1, L1).** Confirm `macos-15-intel`
   runs the build; verify the `--target x86_64-apple-darwin` on `macos-latest` fallback compiles if
   needed.

## Verification (maps to acceptance criteria)

1. **Matrix build** — `prebuild.yml` dispatched: all 7 build legs green, each uploads its
   `paigasus-node-bindings.<platform>.node` artifact.
2. **Dry-run assembly** — the `assemble` job's `napi prepublish --dry-run` (no `--gh-release`) +
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
- **M2 (Medium — musl legs use the image toolchain, not the proto pin) — accepted, made explicit.**
  §2 now states a deliberate decision to accept the Alpine image's Rust for the musl legs
  (leaf-artifact rationale), with a spike check that the image's Rust ≥ MSRV (§6 spike #3).
- **M3 (Medium — `prepublish` gh-release/lerna-tag defaults) — accepted, verified, corrected by the
  spike.** The reviewer (and the napi *docs*) cited a `ghRelease:true` default + a `--no-gh-release`
  flag; the Task-1 spike against the **pinned `@napi-rs/cli` 3.7.2** found **`--gh-release` is an
  opt-in presence flag with no `--no-gh-release`**, and `--tag-style` defaults to `lerna`. So the
  dry-run **omits `--gh-release`** (§2.3, §6) — even safer than pinning a negation. **Deviation from
  the reviewer's "pin `--tag-style` too":** left tag-style + the napi/release-plz division as an
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
