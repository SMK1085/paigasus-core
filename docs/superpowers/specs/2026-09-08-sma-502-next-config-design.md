<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-502 — `@paigasus/next-config`: runtime config, Next factory, and boundary gates

**Issue:** [SMA-502](https://linear.app/smaschek/issue/SMA-502)
**ADR:** ADR-0017 (Console topology & session ownership)
**Design source:** Frontend Architecture Scoping, §§ 2, 6, 7
**Date:** 2026-09-08

---

## 1. Problem

Next.js inlines every `NEXT_PUBLIC_*` value at `next build` time. One OCI image
therefore cannot serve two self-hosters with different IdP issuers or API URLs.
The deploy target is self-hosted containers, so this blocks the product shape.

No shared foundation exists today. `ts/apps/paigasus-console` holds an empty
`next.config.ts`. Nothing makes runtime configuration the default path, and
nothing stops a future app from reaching for `NEXT_PUBLIC_`.

## 2. The principle this package encodes

Build-time inlining is allowed for values that describe the **image**. It is
banned for values that describe the **deployment**.

The zone prefix describes the image. Image A is the IAM zone. The IdP issuer
describes the deployment. Two self-hosters have two different issuers and one
image.

This single rule explains every decision below.

## 3. Scope

A new package `ts/packages/paigasus-next-config` (`@paigasus/next-config`),
`private: true` and source-only, matching the `@paigasus/ui` shape. Plus one new
CI gate, and changes to the console app that consumes the package.

### 3.1 Layout

```
ts/packages/paigasus-next-config/
  package.json          exports ".", "./runtime", "./eslint", "./tsconfig-app"
  moon.yml              id: paigasus-next-config-ts, layer: library
  tsconfig.json
  tsconfig.app.json     the shared Next-app TypeScript preset
  vitest.config.ts
  src/index.ts          createNextConfig()
  src/runtime.ts        defineRuntimeConfig() and the public projection
  src/eslint.mjs        the package-boundary preset
  tests/
```

## 4. The Next config factory

`createNextConfig({ zone, basePath, assetPrefix?, extend? })` returns a
`NextConfig`.

Rules:

- It spreads `extend` first, then forces `output: 'standalone'` last. An app
  cannot override the standalone rule. Without `output: 'standalone'` the image
  ships all of `node_modules`.
- `assetPrefix` defaults to `basePath`. Static chunks must not collide between
  zones behind one origin.
- It throws when `extend` carries an `env` key. The factory owns `env:`.
- It sets `transpilePackages` for the source-only workspace packages, because
  those packages export TypeScript source.
- It writes `env: { PAIGASUS_COMPILED_ZONE, PAIGASUS_COMPILED_BASE_PATH }`.

### 4.1 Why the zone prefix is build-time

`basePath` and `assetPrefix` are `next.config` values, and `next.config` is read
at build time. Next 16 has no runtime `basePath`. So the zone prefix is baked
into the image.

This is correct under the § 2 principle. The prefix describes the image, not the
deployment. The app declares it as a literal in its own `next.config.ts`, which
is committed source.

**Accepted limitation.** A self-hoster who remaps the ingress prefix must rebuild
the image. § 4.2 detects that condition instead of hiding it.

Two alternatives were considered and rejected. Reading `basePath` from env at
build time only moves the same build-time value behind an env var, and hides the
fact that it is build-time. Stripping the prefix at the ingress removes
`basePath` entirely, but it reinstates the static-chunk collision that
`assetPrefix` exists to prevent, and it makes an ingress rewrite load-bearing for
correctness.

### 4.2 The compiled-versus-deployed cross-check

The factory inlines the compiled zone and base path through `env:`. Both values
describe the image, so inlining them is correct.

At first request the runtime module compares `zones[PAIGASUS_ZONE]` against
`PAIGASUS_COMPILED_BASE_PATH`. A mismatch throws, and the message names both
values.

Without this check, an ingress remap produces an intermittently broken navigation
item. ADR-0017 already records that failure as hard to diagnose and invisible to
unit tests.

## 5. The `/runtime` entry

### 5.1 Variables this package owns

| Variable | Meaning | Required |
|---|---|---|
| `PAIGASUS_ZONE` | This app's zone id, e.g. `iam` | yes |
| `PAIGASUS_ZONES` | JSON map of zone id to base path | yes |

Neither has a default. A default hides a misconfiguration.

Helm renders `PAIGASUS_ZONES` from the same values block as the ingress rules,
per ADR-0017. One block, so the two cannot drift.

### 5.2 Composition

The app calls `defineRuntimeConfig(extraShape)` once, in its own module.
`@paigasus/auth` and `@paigasus/sdk` will each export a zod shape for the
variables they own. The app composes those shapes.

One schema, one parse, one place. There is no registry. A registry would let two
packages parse the same env twice and disagree.

`defineRuntimeConfig` returns `{ getRuntimeConfig, getPublicConfig }`.

### 5.3 First request, not module scope

`getRuntimeConfig()` memoizes the parsed result on first success.

It throws when `process.env.NEXT_PHASE === 'phase-production-build'`.

A module-scope read from a prerendered page then fails the build with an
actionable message. Lazy-by-convention alone lets that read through in silence,
and the value it bakes in belongs to whoever ran the build.

The memo caches success only. A failed parse throws on every call. A misconfigured
container therefore fails every request, not only the first.

### 5.4 The client-safe slice

`getPublicConfig()` runs the full config through a separate `publicConfigSchema`.
That schema holds `zone` and `zones` only.

Zod strips unknown keys. So a secret added to an extra shape cannot reach the
browser unless someone edits `publicConfigSchema` deliberately.

The sanitizer is an allowlist by construction, not a denylist. A denylist fails
open when a new field appears.

A test pins `publicConfigSchema`'s key list by strict equality. Widening the
client slice is therefore always a deliberate act.

Internal service URLs are cluster-internal DNS. They must never reach the browser.

### 5.5 Error messages

Every failure throws an `Error` naming the variable key and the failure kind.

**No error message prints an env var's value.** The extra shapes will hold client
secrets. A zod error that echoes the input would put a secret in a log.

## 6. The `NEXT_PUBLIC_` ban gate

New Moon task `repo:next-public-free`, script `ci/next-public/run.sh`. The name
follows `repo:wasm-getrandom-free`.

### 6.1 What it scans

`git ls-files -- ts/`, searching for the literal `NEXT_PUBLIC_`.

Excluded: `ts/pnpm-lock.yaml`, and `**/*.md`. Markdown is excluded so a document
can name the string it bans. A document compiles into nothing.

Anything else needs an `ALLOW_NEXT_PUBLIC` entry with a stated reason.

Untracked paths are out of scope, so `node_modules` needs no rule. A third-party
dependency that uses the prefix is unaffected.

**The ban covers packages, not only apps.** The issue text says "banned in apps".
A source-only package is transpiled by the app and inlines the same way, so an
apps-only ban leaves a hole.

### 6.2 Proving the gate bites

The scan lives in a `scan_tree <dir>` helper.

- `--self-test` drives that helper over synthetic trees in a `mktemp -d`: one
  clean, one violating, one holding the string in Markdown only.
- `--negative-control` asserts the gate reports red on a violating tree.

Both run before the real scan, under an explicit `set -euo pipefail`.

A self-test over an extracted helper cannot see its own call site being deleted.
So `NEXT_PUBLIC_FREE_SH_CALL_SITES` in `ci/affected-graph/ci_targets.py` pins the
real invocation line, the same shape as `RUFF_SH_CALL_SITES`.

### 6.3 Registration — seven obligations

CLAUDE.md records that missing some of these reds nothing at all. So all seven
are explicit tasks in the plan.

1. `ci.yml`'s `T=(…)` array.
2. The marker-delimited command in `CLAUDE.md`.
3. `SELF_SCHEDULED_GATES` — the four `moon.yml` invocation lines
   (`set -euo pipefail`, `--self-test`, `--negative-control`, the real run).
4. `SELF_TASK_EXPECTED_GLOBS` — the literal `inputs`.
5. `NEXT_PUBLIC_FREE_SH_CALL_SITES` — the script pin.
6. `REQUIRED_REPO_TASKS` — the floor for a gate that carries a negative control.
7. `T_AFFECTED_SMOKE_REQUIRED_INPUTS` in `ci/actionlint/run.sh`, plus
   `ci/next-public/**/*` among `repo:affected-smoke`'s own `inputs` in `moon.yml`.

Obligation 7 is what makes the others reachable. Without it, a PR touching only
`ci/next-public/**` does not schedule `repo:affected-smoke`, so no pin can fire
on exactly the PR that breaks it.

### 6.4 Gate placement

The gate is a `repo:*` Moon task with `ts/**` inputs, not an unconditional
`ci.yml` step.

Reason: it then runs under a local `moon ci` before a push, and
`repo:input-liveness` proves its glob stays live.

**Residual, stated plainly.** `repo:input-liveness` catches a **dead** glob. It
does not catch a **too-narrow** one. A future narrowing of this gate's `inputs`
would switch it off, and nothing would report that. The alternative — an
unconditional `ci.yml` step — closes that residual but loses local coverage.
The trade was made deliberately.

## 7. The eslint boundary preset

`@paigasus/next-config/eslint` exports a flat-config array. It uses the core
`no-restricted-imports` rule, scoped by `files:` globs. It adds no dependency.

Entries:

| Scope | Denied | Reason |
|---|---|---|
| `packages/paigasus-ui/**` | `next`, `next/*`, every `@paigasus/*` | § 6 rule 1: plain React, testable in jsdom |
| `packages/paigasus-sdk/**` | every `@paigasus/*` except `@paigasus/proto`; `react`, `react-dom`, `next/*` | server-only, `sdk → proto` |
| `packages/paigasus-app-shell/**` | `@paigasus/sdk`, `@paigasus/auth/server` | the shell is client-reachable |
| `apps/**` | `@paigasus/proto` | apps reach the contract through `sdk` |

The `app-shell` and `auth` entries are written now and are inert until SMA-506
and SMA-508 land. They are still tested — see § 9.

### 7.1 Deviation from the § 6 diagram

The design diagram reads `apps → app-shell → { ui, auth/client }`. This preset
allows an app to import `@paigasus/ui` directly.

Forcing every component through an app-shell re-export barrel buys no safety and
costs a barrel. The rules that carry real safety are the four above.

### 7.2 What eslint cannot do

The client-versus-server boundary is a `'use client'` property, not an
import-path property. `import 'server-only'` at the `sdk` and `auth/server` entry
points is what enforces it.

Those packages do not exist yet. This issue ships the eslint half only.

## 8. Moon and console changes

### 8.1 `.next/cache` exclusion (AC4)

`paigasus-console-ts:build` becomes
`outputs: ['.next/**/*', '!.next/cache/**/*']`.

Moon 2.5.3's handling of a negated output glob is verified in the plan's first
task. If it does not honour one, `outputs` enumerates the kept `.next` subpaths
instead.

### 8.2 The console build's inputs

`next.config.ts` now imports the factory. So the build reads
`@paigasus/next-config` sources, and
`/ts/packages/paigasus-next-config/src/**/*` joins the task's `inputs`.

Without that input, editing the factory serves a cached console build. That is
the exact staleness class SMA-519 paid for.

`ts/pnpm-lock.yaml` is deliberately **not** added to that task.
`repo:next-env-drift` keys on the lockfile precisely because the build does not,
and its comment records why. Changing that premise is a separate decision.

### 8.3 The standalone assertion (AC1)

`paigasus-console-ts:build` becomes a `script:` that runs `next build`, then
fails with a named message when `.next/standalone/server.js` is absent.

A unit test on the returned config object proves the factory returns
`output: 'standalone'`. It does not prove Next honoured it. A Next version that
renames or ignores the key would pass a unit test and ship a broken image.

### 8.4 Affected-graph impact

**No existing `ci/affected-graph/run.sh` case changes.** Every `run_case` and
`run_task_case` touches a Rust or proto file, and `--downstream deep` from those
never reaches a new `ts/packages/*`. `lockfile->all-lint` filters on tasks keyed
to `rs/Cargo.lock`, which this package is not.

The § 7.1 landmine in the design document — strict equality redding when
`sdk → proto` lands — is real and belongs to SMA-508.

### 8.5 Dev ergonomics

The console gets a `.env.local.example` documenting both variables. `next dev`
reads `.env.local`, which stays untracked.

## 9. Testing

### 9.1 Factory

- `output` is `'standalone'`.
- `extend` cannot override `output`.
- `assetPrefix` defaults to `basePath`.
- An `extend.env` key throws.
- The compiled zone and base path reach `env`.

### 9.2 Runtime

- A valid env parses.
- An invalid env throws, and the message names the variable.
- The second call returns the memoized value after `process.env` is mutated.
  This proves the memo rather than assuming it.
- `NEXT_PHASE=phase-production-build` throws.
- A zone-map-versus-compiled-basePath mismatch throws, naming both values.
- An extra shape carrying a secret does not appear in `getPublicConfig()`.
- No error message contains a variable's value.
- `publicConfigSchema`'s key list is pinned by strict equality.

### 9.3 Boundaries

ESLint's Node API runs the real exported preset over `lintText`, with synthetic
file paths. Synthetic paths mean the `app-shell` and `auth` entries are tested
now, before those packages exist.

- Each denied import reports `no-restricted-imports`.
- Each allowed import reports nothing. A rule that reports everything is not a
  boundary rule.

### 9.4 Guard the guard

One test asserts `ts/eslint.config.js` actually consumes the preset.

Without it, deleting the import from `eslint.config.js` leaves every boundary
test green. A fixture table exercises a verdict function; it never exercises the
invocation.

### 9.5 Why no fixture directory

AC3 says "a test fixture". A real fixture directory of deliberately-wrong imports
would have to be excluded from the workspace `ts:lint` and `ts:fmt` runs. An
excluded fixture is exactly the kind of thing that goes inert unnoticed.

In-memory `lintText` needs no exclusion and tests the same shipped config.

## 10. Risks and open verification

Two items are verified in the plan's first task, each with a stated fallback so
implementation does not stall.

1. **Loader question.** Can `next.config.ts` and `ts/eslint.config.js` import
   TypeScript source from the workspace package? If either cannot, that entry
   ships as `.mjs` with a hand-written `.d.mts`. The preset is written as `.mjs`
   by default for this reason: it is configuration data, and TypeScript buys
   little there.
2. **Moon negated outputs.** Does Moon 2.5.3 honour `'!.next/cache/**/*'`? If
   not, enumerate the kept subpaths.

Further risks:

- **`zod` is a new catalog entry.** pnpm 11's 24-hour `minimumReleaseAge` reds CI
  on a same-day npm release. Pin a version at least one day old.
- **The `repo:next-public-free` inputs residual** — see § 6.4.

## 11. Out of scope

Each item is owned elsewhere.

- Renaming `paigasus-console` to `iam-console` — SMA-511.
- `import 'server-only'` wiring — SMA-506, SMA-508.
- Capability keys in the public slice — ADR-0020.
- The `ci/affected-graph` strict-equality re-baseline — SMA-508.
- Containerization and Helm charts — design § 7.1.

## 12. Acceptance criteria mapping

| AC | Where it is satisfied |
|---|---|
| 1. An app builds to a standalone server reading config at runtime | § 4, § 5, § 8.3 |
| 2. New CI gate: `NEXT_PUBLIC_` is banned | § 6 |
| 3. The boundary rule fails a deliberately-wrong import | § 7, § 9.3 |
| 4. Moon `build` outputs exclude `.next/cache` | § 8.1 |
