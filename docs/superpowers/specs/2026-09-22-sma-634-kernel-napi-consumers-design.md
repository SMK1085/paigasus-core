# SMA-634 — Load the kernel napi binding in Node consumers

**Linear:** [SMA-634](https://linear.app/smaschek/issue/SMA-634/ts-paigasuskernel-napi-binding-never-reaches-node-modules-files)
**Related:** SMA-511 (spec § 4.7, D6), SMA-512, SMA-513 (spec F5)
**ADR:** ADR-0005 (cross-language behavior lives once in `paigasus-kernel`)
**Date:** 2026-09-22
**Revision:** 1, before the adversarial challenge. § 12 records the challenge.

---

## 1. Problem

`@paigasus/kernel` imports `@paigasus/node-bindings` under the `node` export condition. That
package is a pnpm `file:` dependency on `rs/crates/bindings/paigasus-node-bindings`. pnpm copies a
`file:` target one time, at install time, and it copies only the paths in the `files` allowlist.
The allowlist is `["index.js", "index.d.ts"]`. So the `.node` binary never reaches `node_modules`.

A Next 16 app that imports the kernel from a server component fails `next build` at "Collecting
page data" with `Cannot find native binding`. No Node consumer outside the kernel's own tests can
load the binding.

Because of this defect, SMA-511 added a hand-written PRN reader to the consoles. It is a recorded
ADR-0005 exception. It now lives in `ts/packages/paigasus-console-core/src/prn-tenancy.ts`.

This design fixes the packaging defect, proves the fix in a Next standalone build, and removes the
ADR-0005 exception.

---

## 2. As-built findings (measured 2026-09-22)

Read from the tree at `8c1297b7`.

### F1 — The published package model is correct and must not change

The main `@paigasus/node-bindings` npm package is **loader-only**. Seven per-platform packages carry
the binaries (`napi create-npm-dirs` and `napi artifacts` in `.github/workflows/prebuild.yml`,
`napi prepublish` in `.github/workflows/release.yml`). `prebuild.yml` asserts that the main tarball
contains no `.node` file. So `*.node` in the `files` allowlist is not the fix: it can put a binary
into the main tarball when a `.node` file is in the crate directory at pack time.

### F2 — The defect is in the monorepo link only

`ts/packages/paigasus-kernel/package.json` declares both bindings as `file:` dependencies. pnpm
links a `file:` target at install time. `.github/workflows/ci.yml` runs `pnpm --dir ts install
--frozen-lockfile` as a workflow step before any Moon task. `paigasus-kernel-ts:build` writes the
`.node` and `.wasm` files after that step. So the build output never reaches the installed copy.

### F3 — The SMA-511 wasm spike failed for the same reason

SMA-511 § 13 row 10: the wasm route passed in a warm tree (B1). It failed in the CI state (B2) with
`Can't resolve './paigasus_wasm_bg.wasm'`, because the `.wasm` file did not exist at install time.
The spike did not show that wasm itself cannot work in Next.

### F4 — The kernel's own tests hide the defect

`ts/packages/paigasus-kernel/vitest.config.ts` aliases `@paigasus/node-bindings` to the crate
directory. So the kernel tests never use the path that a consumer uses.

### F5 — Every caller of `prn-tenancy` runs on the server

`console-core/src/index.ts` and `prn-tenancy.ts` import `server-only`. The callers are server
components and `server-only` modules in `iam-console` and `gateway-console`. Neither `proxy.ts`
imports `@paigasus/console-core`, and an ESLint boundary rule (`paigasus/boundaries/app-middleware`)
enforces that. So Next resolves the kernel under the `node` condition, which selects the napi entry.

### F6 — The kernel has PRN grammar only

The kernel exports `prnBuild`, `prnCanonicalize`, `prnErrorKind`, `prnService`, `prnRegion`,
`prnOrg`, `prnResourceType`, `prnResourceId` and others. Every function except `prnErrorKind` and
`prnOrg` (for an absent org) throws an `Error` whose message is the PRN error kind.

The kernel has no concept of IAM tenancy. It has no `TenancyRef`, no org-field rule per kind, no
IAM root PRN and no UUID predicate. These stay hand-written.

### F7 — The console image is pure TypeScript today

`ts/Dockerfile` uses `ts/` as its build context (`COPY . /build/`). It has no Rust toolchain. The
SMA-513 spec F5 states that the console image needs no Rust, no napi and no wasm. No CI script
asserts F5.

### F8 — No gate reads the `file:` specifiers

A search of `ci/`, `.github/` and `.moon/` finds no script that reads the `file:` specifiers. Only
`ts/pnpm-lock.yaml` records them (`@paigasus/node-bindings@file:../rs/crates/bindings/…`).

### F9 — Next config

`createNextConfig()` (`ts/packages/paigasus-next-config/src/index.ts`) already lists
`@paigasus/kernel` in `transpilePackages`. It sets `output: 'standalone'`. Each app passes
`outputFileTracingRoot` as `ts/`. The factory sets no `serverExternalPackages`.

---

## 3. Decisions

| # | Topic | Decision |
| -- | -- | -- |
| D1 | The link | Both kernel binding dependencies change from `file:` to `link:`. The `files` allowlists, `prebuild.yml` and `release.yml` do not change (F1) |
| D2 | The Node route | Node consumers use the napi entry. The kernel gets no `./wasm` subpath export |
| D3 | Delivery | Two PRs under SMA-634. PR 1 fixes the link and proves it. PR 2 moves `console-core` to the kernel and changes the console image |
| D4 | ADR-0005 | PR 2 removes the exception. `prn-tenancy.ts` becomes an IAM adapter over the kernel. Its public API does not change |
| D5 | CI cost | The consoles depend on the full `paigasus-kernel-ts:build` (napi and wasm-pack). PR 2 measures the time increase. We do not add a separate `build-napi` task now |
| D6 | The image | `ts/Dockerfile` gets a named build context `rs` and a `napi` stage that compiles the binding on bookworm |

---

## 4. PR 1 — the packaging fix and its proof

### 4.1 The link (D1)

In `ts/packages/paigasus-kernel/package.json`:

```json
"@paigasus/node-bindings": "link:../../../rs/crates/bindings/paigasus-node-bindings",
"@paigasus/wasm": "link:../../../rs/crates/bindings/paigasus-wasm"
```

pnpm makes a symlink to the crate directory. It copies nothing, so the `files` allowlist does not
apply. A binary that `paigasus-kernel-ts:build` writes after the install is visible through the
symlink.

pnpm does not install the dependencies of a `link:` target. Neither binding has runtime
dependencies, so this has no effect. The regenerated `ts/pnpm-lock.yaml` records `link:`.

### 4.2 Remove the test workaround

Remove the `@paigasus/node-bindings` alias from `ts/packages/paigasus-kernel/vitest.config.ts`. The
`node` project then resolves through `node_modules`, which is the path a consumer uses.

The `browser` project keeps its `@paigasus/kernel` alias to `./src/wasm.ts`. That alias exists
because vitest forces the `node` condition, not because of the copy (F4).

### 4.3 The proof: a Next standalone fixture

A small Next app at `ts/packages/paigasus-kernel/tests/next-consumer/`.

- The path must not contain a directory named `build/`. The root `.gitignore` ignores every
  `build/` directory, so a new file there is never committed.
- No file base name may be a Windows reserved device name (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`,
  `LPT1`–`LPT9`). A file named `prn.ts` is therefore not permitted.
- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- The app uses `createNextConfig()` from `@paigasus/next-config`, so the proof uses the same config
  as the consoles.
- It has one route handler. The handler calls `prnBuild`, `prnOrg`, `prnResourceType` and
  `prnResourceId` from `@paigasus/kernel` and returns the results as JSON.

A new Moon task, `paigasus-kernel-ts:test-next`, does these steps in this order:

1. It depends on `~:build`, so the `.node` file exists.
2. It runs `next build` on the fixture.
3. It asserts that the standalone tree contains `paigasus-node-bindings.<platform>.node`.
4. It copies the standalone tree to a temporary directory **outside the repository**. A symlink
   back into `rs/` then cannot make the test pass by accident.
5. It starts `server.js` from that copy, calls the route, and compares each value with the
   expected value from the parity corpus (`rs/crates/libs/paigasus-kernel-parity/vectors/`).
6. It stops the server. Every exit path stops the server.

The task joins the CI target list only if it is a new task name. `test-next` is a new name, so PR 1
adds `:test-next` to `ci.yml`'s `T=(…)` array and to the root `CLAUDE.md` `ci-targets` block
together. `repo:affected-smoke` checks that the two agree. The alternative is to run the fixture
inside the existing `test` task. The plan chooses one and records why.

### 4.4 Red first

Before PR 1 changes the link, the plan runs `paigasus-kernel-ts:test-next` one time on the
unchanged `file:` link. The expected result is a failure with `Cannot find native binding`, or a
missing `.node` file at step 3. The plan records the output. This proves that the fixture detects
the defect.

After the link change, the plan runs the task again. The expected result is a pass.

### 4.5 Measurement first: the tracing root

The binary is under `rs/`. `outputFileTracingRoot` is `ts/`. Plan task 1 measures:

- whether the standalone tree contains the `.node` file when its real path is outside the tracing
  root;
- whether Next bundles the napi loader or leaves it external, and whether
  `serverExternalPackages: ['@paigasus/node-bindings']` is necessary.

If `serverExternalPackages` is necessary, PR 1 adds it to `createNextConfig()`, so every zone gets
it.

**Stop rule.** If the standalone tree does not contain the binary, the implementer **stops and
reports** to Sven before any fallback. Candidate fallbacks are an `outputFileTracingIncludes` entry
or an explicit copy step. None of them is approved by this spec.

### 4.6 Records

- `ts/CLAUDE.md`: correct the SMA-634 entry. It states the `link:` fix, the fixture task, and that
  the consoles still use the ADR-0005 exception until PR 2.
- `ts/packages/paigasus-kernel/package.json`: update the `_comment_*` fields that describe the
  `file:` dependencies.

---

## 5. PR 2 — `console-core` uses the kernel

### 5.1 The dependency edges

- `ts/packages/paigasus-console-core/package.json` gets `"@paigasus/kernel": "workspace:*"` in
  `dependencies`.
- `ts/packages/paigasus-console-core/moon.yml` gets `paigasus-kernel-ts` in `dependsOn`.
- `console-core`, `iam-console` and `gateway-console` get an explicit dep on
  `paigasus-kernel-ts:build` for `build`, `typecheck`, `test` and `test-e2e`, where those tasks
  exist. This is the pattern that these files already use for `contracts:generate`.
- The three vitest configs already request the `node` condition. Through the `link:` symlink they
  load the real `.node` file. They need no alias.

### 5.2 `prn-tenancy.ts` becomes an IAM adapter (D4)

The exports, the `null` and `TypeError` behavior, and the lower-case ids do not change. No caller
in the two apps changes.

| Export | New source |
| -- | -- |
| `parseTenancyPrn(prn)` | If `prnErrorKind(prn) !== ''`, return `null`. Otherwise read `prnService`, `prnRegion`, `prnOrg`, `prnResourceType` and `prnResourceId`. Then apply the IAM rule without change: service `iam`; an empty region; resource type `organization`, `team` or `project`; no org for an organization; an org for a team or a project. Ids are lower-case |
| `organizationPrn`, `teamPrn`, `projectPrn` | `requireUuid` first (a `TypeError` for a bad id, as today), then `prnBuild('iam', '', org, kind, id)` |
| `ROOT_PRN` | Stays a constant. The test against the live Rust `root_prn()` stays |
| `isUuid` | Stays a local regex. It guards the shape of a URL segment. It is not PRN grammar, and the kernel has no UUID predicate (F6) |

The file removes `MAX_LEN` and the hand-written field grammar, because the kernel enforces them.
The header comment no longer states an ADR-0005 exception. It states that the file holds the IAM
tenancy rule over the kernel grammar.

A non-empty region stays a `null` result. This is IAM policy, not grammar: `TenancyRef` has no
region field, so a regionful PRN would lose its region on the way back out.

`prnBuild` can throw for an input that `requireUuid` accepts only if the kernel grammar changes. The
builders do not catch that error. It surfaces as a thrown `Error` with the kernel error kind.

### 5.3 Tests

- `tests/unit/prn-tenancy.test.ts` keeps the parity corpus replay, the builder round trips, the
  `ROOT_PRN` checks and the negative table. The tests now check the adapter and its tenancy rule.
- One new case: a corpus row with a non-empty `error_kind` that the old regex path did not check
  (for example a grammar violation in a field that the old reader skipped). The plan selects the row
  from `prn_canonical.json` and asserts `null`.
- The consoles' existing unit, integration and e2e tests run against the real napi binding. They
  must pass without change.

### 5.4 Records

- `ts/CLAUDE.md`: remove the entry for the "fallback C" exception. State that the consoles use the
  kernel through napi, and that their tasks depend on `paigasus-kernel-ts:build`.
- This spec supersedes the SMA-511 § 12 D6 limit and the SMA-513 F5 finding. Those specs are
  historical records. PR 2 does not edit them.

---

## 6. PR 2 — the console image and CI (D6)

### 6.1 A named build context

`ci/images/run.sh build_console_one` adds `--build-context rs=rs` to `docker buildx build`. The ts
build context does not change, so no existing path in `ts/Dockerfile` moves. In the builder,
`/build` is `ts/`, so the `link:` target `../../../rs/…` resolves to `/rs/…`.

### 6.2 A `napi` stage

The stage copies the pattern of `rs/Dockerfile`:

- `FROM rust:1.95.0-bookworm@sha256:…`, with the same digest as `rs/Dockerfile`, and
  `ENV RUSTUP_TOOLCHAIN=1.95.0`. Bookworm has glibc 2.36. The runtime is
  `distroless/nodejs24-debian12`, also glibc 2.36. A builder on the Ubuntu 24.04 runner (glibc
  2.39) is not permitted.
- `COPY --from=rs . /src`, then `cargo build --release --locked -p paigasus-node-bindings` with
  cache mounts for the registry and `target/`.
- In the same `RUN`, copy `target/release/libpaigasus_node_bindings.so` to
  `/out/paigasus-node-bindings.linux-<arch>-gnu.node`. A cache mount is not part of the layer, so
  the copy must be in the same `RUN`.
- `dpkg --print-architecture` gives the arch. `amd64` maps to `x64`. `arm64` maps to `arm64`. Any
  other value stops the build with a message. These are the names that the napi loader expects.

### 6.3 The builder stage

- `COPY --from=rs crates/bindings/paigasus-node-bindings /rs/crates/bindings/paigasus-node-bindings`
  and the same for `paigasus-wasm`. `pnpm install` needs both `link:` targets.
- `COPY --from=napi /out/*.node` into `/rs/crates/bindings/paigasus-node-bindings/`, beside
  `index.js`.
- The existing steps run without change: install, `next build`, staging.
- A new assertion, beside the `server.js` check: the standalone tree contains
  `paigasus-node-bindings.linux-<arch>-gnu.node`.

### 6.4 Smoke tests in `ci/images/run.sh`

- Against each final image, run `docker run --rm --entrypoint /nodejs/bin/node <image> -e "<script>"`.
  The script loads the traced binding from the standalone tree and calls `prnBuild`. It must print
  the expected canonical PRN. This detects a missing file, a wrong arch and a glibc mismatch in CI,
  not at container start.
- `assert_console_pins` also asserts that the Rust image digest and `RUSTUP_TOOLCHAIN` in
  `ts/Dockerfile` are the same as in `rs/Dockerfile`.

### 6.5 The `images.yml` path filter

The `pull_request` path filter adds `rs/crates/libs/paigasus-kernel/**`,
`rs/crates/bindings/paigasus-node-bindings/**`, `rs/Cargo.lock` and `rs/rust-toolchain.toml`, if
that file exists. A kernel change then rebuilds the console image on the PR.

### 6.6 Build time and arm64

Each console image compiles the binding crate once per arch. The PR runs amd64 only. Before the
merge, the implementer runs `gh workflow run images.yml --ref <branch>` and records the arm64
result.

---

## 7. Error handling

Every new step fails closed.

- A missing `.node` file in the fixture's standalone tree fails `test-next` (§ 4.3 step 3).
- An unknown arch stops the `napi` stage (§ 6.2).
- A missing `.node` file in the image's standalone tree stops the image build (§ 6.3).
- A load error or a wrong value stops the image smoke test (§ 6.4).
- No step copies a binary from the host into an image or a standalone tree.
- A kernel error in `parseTenancyPrn` becomes `null`. A bad id in a builder becomes a `TypeError`.
  This is the current contract.

---

## 8. Measurements to take

| # | When | What | Rule |
| -- | -- | -- | -- |
| M1 | PR 1, task 1 | Does the standalone tree contain the `.node` file when its real path is outside `outputFileTracingRoot`? Is `serverExternalPackages` necessary? | If the binary is absent, **stop and report** (§ 4.5) |
| M2 | PR 1 | The red-first run on the `file:` link (§ 4.4) | Must fail before the fix and pass after it |
| M3 | PR 2, task 1 | Which `.dockerignore` applies to the named context `rs`? `target/` and local `*.node` files must not enter it | If they enter, add an ignore file for that context and measure again |
| M4 | PR 2 | Does `cargo auditable build` work for the cdylib, and does the image SBOM then list the Rust crates? | If not, use `cargo build` and record the SBOM gap in § 9 |
| M5 | PR 2 | The arm64 image result through `workflow_dispatch` (§ 6.6) | Required before the merge |
| M6 | PR 2 | The CI time increase for an affected console run (D5) | Recorded in the PR. A `build-napi` split is a separate decision |

---

## 9. Recorded limits

- The console tasks wait for the wasm-pack compile, although the consoles need only napi (D5).
- The console image build now compiles Rust. It takes longer, and a Rust toolchain change can red
  the console image.
- `isUuid`, `ROOT_PRN` and the IAM tenancy rule stay hand-written. They are IAM domain rules, not
  kernel grammar, so they are not an ADR-0005 exception.
- A future browser consumer of the kernel still has the wasm route only through the `browser`
  condition. This design does not prove the wasm route in a Next client bundle.

---

## 10. Files touched

**PR 1**

- `ts/packages/paigasus-kernel/package.json`
- `ts/packages/paigasus-kernel/vitest.config.ts`
- `ts/packages/paigasus-kernel/moon.yml`
- `ts/packages/paigasus-kernel/tests/next-consumer/**` (new)
- `ts/pnpm-lock.yaml`
- `ts/packages/paigasus-next-config/src/index.ts` (only if M1 shows that `serverExternalPackages` is necessary)
- `.github/workflows/ci.yml` and the root `CLAUDE.md` `ci-targets` block (only for a new task name)
- `ts/CLAUDE.md`

**PR 2**

- `ts/packages/paigasus-console-core/package.json`, `moon.yml`, `src/prn-tenancy.ts`,
  `tests/unit/prn-tenancy.test.ts`
- `ts/apps/iam-console/moon.yml`, `ts/apps/gateway-console/moon.yml`
- `ts/pnpm-lock.yaml`
- `ts/Dockerfile`
- `ci/images/run.sh`
- `.github/workflows/images.yml`
- `ts/CLAUDE.md`

---

## 11. Acceptance

- PR 1: `paigasus-kernel-ts:test-next` fails on the `file:` link and passes on the `link:` link. The
  standalone server answers from a copy outside the repository with the corpus values.
- PR 1: the published npm model is unchanged. `prebuild.yml`'s loader-only assertion still passes.
- PR 2: `prn-tenancy.ts` imports the kernel and holds no PRN grammar. `paigasus-console-core-ts:test`
  passes against the real napi binding.
- PR 2: both console images build on amd64 and arm64, and the smoke test loads the binding and
  prints the expected PRN.
- PR 2: the full gate graph in the root `CLAUDE.md` passes.

---

## 12. Challenge log

This section records the Stage 2 adversarial challenge.
