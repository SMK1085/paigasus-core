<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-502 — `@paigasus/next-config`: runtime config, Next factory, and boundary gates

**Issue:** [SMA-502](https://linear.app/smaschek/issue/SMA-502)
**ADR:** ADR-0017 (Console topology & session ownership)
**Design source:** Frontend Architecture Scoping, §§ 2, 6, 7
**Date:** 2026-09-08
**Revision:** 2 — after the adversarial spec challenge. § 14 records what changed.

**Versions this spec is written against.** Next 16.3.4, React 19.2.8, TypeScript 6.0.3,
zod 4.5.4, Moon 2.5.3, pnpm 11. Next 16 makes Turbopack the default `next build`
bundler, so every Next behaviour measured in § 13 must be measured on that path.

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

Every new source file carries an SPDX header, per CLAUDE.md. This issue adds
roughly ten.

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
- It throws when `extend` carries an `env` key. The factory owns `env:`, because
  `env:` is a build-time inlining channel equivalent to `NEXT_PUBLIC_`.
- It sets `outputFileTracingRoot` explicitly. See § 4.3.
- It sets `transpilePackages` for the source-only workspace packages, because
  those packages export TypeScript source.
- It writes `env: { PAIGASUS_COMPILED_ZONE, PAIGASUS_COMPILED_BASE_PATH }`.
- `assetPrefix` is passed through only when the caller supplies it. See § 4.4.

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

At first request the runtime module runs two comparisons.

1. `PAIGASUS_ZONE` must equal `PAIGASUS_COMPILED_ZONE`. This detects the IAM
   image deployed as some other zone.
2. `zones[PAIGASUS_ZONE]` must equal `PAIGASUS_COMPILED_BASE_PATH`. This detects
   an ingress remap.

Without these, a mismatch produces an intermittently broken navigation item.
ADR-0017 already records that failure as hard to diagnose and invisible to unit
tests.

**The check is fail-closed.** An absent or empty `PAIGASUS_COMPILED_ZONE` throws
a named error. A fail-open check is worse than none, because this document then
records that the condition is detected when it is not.

**Both sides are normalised before comparison.** One canonical form: a leading
`/`, no trailing `/`, and `''` for a zone mounted at the origin root, because
Next rejects `basePath: '/'`. `/iam`, `/iam/` and `iam` all normalise to `/iam`.
Without this, a trailing slash in the operator's JSON hard-fails a correctly
configured deployment.

### 4.3 `outputFileTracingRoot`

With `output: 'standalone'`, Next writes the traced server tree relative to
`outputFileTracingRoot`, which it infers from the nearest lockfile. Here that is
`ts/pnpm-lock.yaml`, so the inferred entry point is very likely
`.next/standalone/apps/paigasus-console/server.js`, not
`.next/standalone/server.js`.

The factory therefore sets `outputFileTracingRoot` explicitly, and § 8.3 derives
the asserted path from the same value rather than hard-coding one. The real
emitted path is measured in § 13 M2.

### 4.4 `assetPrefix` is not defaulted

Revision 1 defaulted `assetPrefix` to `basePath`, justified as preventing
static-chunk collisions between zones. **That justification was wrong.** With
`basePath: '/iam'`, Next already serves chunks under `/iam/_next/`, so `basePath`
alone prevents the collision. `assetPrefix` exists for CDN offload, which is a
different axis. The default was redundant.

`assetPrefix` is therefore passed through only when the caller supplies it.

**Revision 2 also feared a double prefix, and that fear is now DISPROVEN.**
Revision 2 argued that if Next composed the two keys, the default would produce
`/iam/iam/_next/…` and every chunk would 404. Measured on Next 16.3.4
(§ 13 M3): with `basePath: '/iam'` alone the emitted asset URL is
`/iam/_next/static/…`, and with `basePath` **and** `assetPrefix` both set to
`/iam` it is still `/iam/_next/static/…`. The two do not concatenate;
`assetPrefix` is an independent override of where static assets are fetched
from. So the redundancy argument stands on its own and the 404 hazard does not
exist. The measurement is recorded rather than the prediction.

## 5. The `/runtime` entry

`@paigasus/next-config/runtime` opens with `import 'server-only'`.

In the edge runtime Next does not provide a dynamic `process.env`; values must be
known at build time. Middleware or an edge route importing this module would
therefore read inlined or undefined values, not deployment values — the same
silent-wrong-value class § 4.2 exists to prevent, arriving through a different
door.

**Two doors, two guards. `server-only` closes only one of them.** Revision 2
claimed `server-only` makes a middleware or edge-route import a build error.
**That claim is DISPROVEN** (§ 13 M6). `server-only` throws only when the
`react-server` export condition is ABSENT, and Next 16.3.4 sets that condition
for the middleware layer too, so the import resolves to `server-only`'s own
`empty.js` there and is a no-op. Measured: a `middleware.ts` importing this
module builds at **exit 0**, and the module lands in
`.next/server/edge/chunks/`.

What `server-only` genuinely does is stop the module reaching a **client
bundle**, which is real and is kept.

The edge door is closed instead by a **`NEXT_RUNTIME` check** in
`getRuntimeConfig()`. Next defines `process.env.NEXT_RUNTIME` as the literal
`'edge'` for the edge compilation (`next/dist/build/define-env.js:80`), so the
branch is resolved statically: measured, the edge chunk carries an
**unconditional throw** and the node chunk carries no guard code at all
(§ 13 M6b). It is a loud runtime failure on first call, not a build error.

**This module is Node-runtime only.** ADR-0017's rule that middleware performs
cookie-presence checks only is what keeps that constraint satisfiable.

`server-only` is a one-line dependency and does not wait for SMA-506.

### 5.1 Variables this package owns

| Variable | Meaning | Required |
|---|---|---|
| `PAIGASUS_ZONE` | This app's zone id, e.g. `iam` | yes |
| `PAIGASUS_ZONES` | JSON map of zone id to base path | yes |

Neither has a default. A default hides a misconfiguration.

Helm renders `PAIGASUS_ZONES` from the same values block as the ingress rules,
per ADR-0017. One block, so the two cannot drift.

`zones` is typed `z.record(z.string(), basePathSchema)`, where `basePathSchema`
is the § 4.2 canonical-form validator. A non-path value in the operator's JSON is
rejected at parse time rather than carried into the client slice. See § 5.4.

### 5.2 Composition

The app calls `defineRuntimeConfig(extraShape)` once, in its own module.
`@paigasus/auth` and `@paigasus/sdk` will each export a zod shape for the
variables they own. The app composes those shapes.

One schema, one parse, one place. There is no registry. A registry would let two
packages parse the same env twice and disagree.

**Key collisions throw.** `defineRuntimeConfig` throws when `extraShape` declares
`PAIGASUS_ZONE` or `PAIGASUS_ZONES`. Leaving this unstated would let two people
build override, merge, and throw.

`defineRuntimeConfig` returns `{ getRuntimeConfig, getPublicConfig }`.

### 5.3 First request, not module scope

`getRuntimeConfig()` memoizes the parsed result on first success.

It throws when `process.env.NEXT_PHASE === 'phase-production-build'`.

A module-scope read from a prerendered page then fails the build with an
actionable message. Lazy-by-convention alone lets that read through in silence,
and the value it bakes in belongs to whoever ran the build.

The memo caches success only. A failed parse throws on every call. A misconfigured
container therefore fails every request, not only the first.

**What is measured, and what is not.** `next build` assigns
`process.env.NEXT_PHASE = 'phase-production-build'` at
`next/dist/build/index.js:1212`, and nothing under `next/dist/server/` assigns
it. Both measured on the pinned Next 16.3.4. **Not measured:** whether the forked
workers that run static generation inherit that assignment. That is the whole
premise of this control, so § 13 M1 proves it end to end rather than assuming
`child_process` env inheritance. § 13 M1 also names the fallback: `connection()`
from `next/server` is Next's supported "this must not prerender" primitive and
does not depend on an env var.

A unit test that sets `NEXT_PHASE` by hand proves the code branch and never the
premise — the same argument § 8.3 makes about `output: 'standalone'`.

### 5.4 The client-safe slice

`getPublicConfig()` runs the full config through a separate `publicConfigSchema`.
That schema holds `zone` and `zones` only.

Zod strips unknown keys. So a secret added to an extra shape cannot reach the
browser unless someone edits `publicConfigSchema` deliberately.

The sanitizer is an allowlist by construction, not a denylist. A denylist fails
open when a new field appears.

**The limit of that argument, stated plainly.** Zod strips unknown *object* keys.
It does not constrain the key set of a `z.record`. So a secret placed inside the
`PAIGASUS_ZONES` JSON rides through `publicConfigSchema` inside an allowed key.
`basePathSchema` (§ 5.1) **narrows** this: it rejects a value that is not a
canonical base path at parse time.

**It does not CLOSE it.** A path-shaped secret is a valid canonical base path.
`{"x": "/sk-live-abc123"}` passes `basePathSchema` and reaches the browser
unchanged. So the residual is "a secret that happens to look like a URL path",
which is a narrower class than "any secret", and nothing here removes it. The
real control against pasting a secret into the zone map is that Helm renders
`PAIGASUS_ZONES` from the same values block as the ingress rules (§ 5.1), so the
map is generated rather than hand-written.

A test pins `publicConfigSchema`'s key list by strict equality. Widening the
client slice is therefore always a deliberate act.

Internal service URLs are cluster-internal DNS. They must never reach the browser.

**Transport.** `getPublicConfig()`'s result is passed as a **prop from a server
component**. This issue ships no inline `<script>` serialization. That path is a
`</script>`-injection hazard when the JSON is not escaped, so whichever issue
first needs it owns the escaping rule. Naming the transport here stops the first
consumer inventing one.

### 5.5 Error messages

Every failure throws an `Error` naming the variable key and the failure kind.

**No error message prints an env var's value.** The extra shapes will hold client
secrets. A zod error that echoes the input would put a secret in a log. On zod 4
this means formatting from `error.issues[].path` and `.code`, never rendering
`.input`.

**A `message` is rendered only for a key this package OWNS.** Rendering every
`code: 'custom'` message verbatim was safe only while this file authored every
custom issue, which is exactly what `extraShape` ends. A `.refine()` supplied by
`@paigasus/auth` or `@paigasus/sdk` raises `code: 'custom'` and may interpolate
its input, so the filter is by path root, not by code. Measured (§ 13 M6c): only
zod 4's `{ error: (iss) => … }` form leaks, so that is the form the test pins.

## 6. The `NEXT_PUBLIC_` ban gate

New Moon task `repo:next-public-free`, `toolchain: 'system'`, script
`ci/next-public/run.sh`. The name follows `repo:wasm-getrandom-free`.

### 6.1 Check 1 — the prefix scan

`git ls-files -- ts/`, searching for the literal `NEXT_PUBLIC_`.

Excluded: `ts/pnpm-lock.yaml`, and `**/*.md`. Markdown is excluded so a document
can name the string it bans. A document compiles into nothing.

Anything else needs an `ALLOW_NEXT_PUBLIC` entry with a stated reason.

Untracked paths are out of scope, so `node_modules` needs no rule. A third-party
dependency that uses the prefix is unaffected.

**The ban covers packages, not only apps.** The issue text says "banned in apps".
A source-only package is transpiled by the app and inlines the same way, so an
apps-only ban leaves a hole.

**A corpus floor.** The scan asserts it collected at least `CORPUS_FLOOR` files,
with a re-baseline message naming the constant. Without it, moving or renaming
`ts/` makes `git ls-files -- ts/` return zero paths and the gate reports a clean
pass forever. § 6.5 records that `repo:input-liveness` cannot reach this case.
`ci/ruff/run.sh:39` and `:120-123` are the precedent.

### 6.2 Check 2 — every app uses the factory

Check 1 alone leaves an open equivalent channel. A hand-written
`ts/apps/*/next.config.ts` carrying its own `env: { ISSUER: process.env.ISSUER }`
contains no `NEXT_PUBLIC_` literal, never reaches the factory's `extend.env`
guard, and bakes a deployment value into the image.

So the gate also asserts that every `ts/apps/*/next.config.ts` imports
`@paigasus/next-config` and calls `createNextConfig`. A reasoned allowlist entry
is the escape.

This is § 9.4's reasoning applied where the consequence is larger.

### 6.3 Proving the gate bites

Both checks live in helpers taking a directory. `--self-test` drives them over
synthetic fixtures; `--negative-control` asserts the gate reports red on a
violating fixture. Both run before the real scan, under an explicit
`set -euo pipefail` in the `moon.yml` `script:` block — Moon does not enable
errexit there, so without it a failing control is masked by the passing real run
(`moon.yml:70-72`).

**Each fixture is a git repository.** Check 1 reads `git ls-files`, so a bare
`mktemp -d` fixture would exercise a different code path from production and
prove nothing about the tracked-file filter or the exclusion list. Every fixture
runs `git -C "$tmp" init -q` and adds its files, mirroring `ci/ruff/run.sh:135-136`
and `:168`.

**Fixtures live in a bare `mktemp -d`, outside the working tree.**
`repo:actionlint` and `repo:input-liveness` carry `inputs: ['**/*']` and hash-walk
the whole tree concurrently under `moon ci`. `ci/ruff/run.sh:208-211` records the
measured reason.

Fixture cases: clean; violating; the string in Markdown only; the string in the
lockfile only; a corpus below the floor; an app config that does not call the
factory; an app config that does.

**Exit-code contract**, matching every other `ci/*/run.sh`: 0 the repo is clean,
1 the repo is wrong, 2 the gate could not run. A failed `git` read is 2, not 1.

**The call-site pin.** A self-test over an extracted helper cannot see its own
production call site being deleted, so `NEXT_PUBLIC_FREE_SH_CALL_SITES` in
`ci/affected-graph/ci_targets.py` pins the real invocation lines — the flag
parse, both check invocations, and both report arms — mirroring
`RUFF_SH_CALL_SITES`. § 6.6 records what that pin actually costs.

### 6.4 Registration — seven obligations

CLAUDE.md records that missing some of these reds nothing at all. So all seven
are explicit plan tasks.

1. `ci.yml`'s `T=(…)` array.
2. The marker-delimited command in `CLAUDE.md`.
3. `SELF_SCHEDULED_GATES` — the four `moon.yml` invocation lines
   (`set -euo pipefail`, `--self-test`, `--negative-control`, the real run).
4. `SELF_TASK_EXPECTED_GLOBS` — the literal `inputs`. `check_gate_inputs`
   compares globs sorted first, then literal files sorted
   (`ci_targets.py:322-324`, `:336-340`), so the entry must be written in that
   order.
5. `NEXT_PUBLIC_FREE_SH_CALL_SITES` — the script pin. See § 6.6 for its real cost.
6. `REQUIRED_REPO_TASKS` — the floor for a gate that carries a negative control.
7. `ci/next-public/**/*` among `repo:affected-smoke`'s own `inputs` in `moon.yml`,
   floored by a `T_AFFECTED_SMOKE_REQUIRED_INPUTS` entry in
   `ci/actionlint/run.sh`.

**Obligation 7's justification, corrected.** Revision 1 claimed that without it a
PR touching only `ci/next-public/**` would not schedule `repo:affected-smoke`.
That is false: `moon.yml:205` already lists `'ci/**/*'` among that task's inputs.
The narrow entry is still added, because the file's own comment (`moon.yml:198-204`)
records that the four narrower `ci/` globs are kept deliberately — check 8e in
`ci/actionlint/run.sh` floors that array at `-ge 20` over 23 entries, so
collapsing narrow globs into the broad one erodes the headroom.

Editing `repo:affected-smoke`'s block means matching its gate-enforced style:
check 8e parses it with a hand-rolled extractor held to task keys at two spaces,
fields at four, `script: |` as a literal block, and `inputs:` as a block sequence
(`ci/actionlint/run.sh:5325-5330`).

`ci/next-public/README.md` ships with the gate, carrying a Limitations section.
Twelve of twelve existing gate directories have one, and several are cited by
name from CLAUDE.md.

### 6.5 Gate placement and inputs

The gate is a `repo:*` Moon task, not an unconditional `ci.yml` step, so it runs
under a local `moon ci` before a push and `repo:input-liveness` proves its globs
stay live.

**The inputs must cover the whole scanned corpus.** The scan is
`git ls-files -- 'ts/'` minus the lockfile and Markdown, so the declared inputs
are the same shape:

```
ts/**/*
!ts/apps/*/.next/**
ci/next-public/**/*
```

`.moon/workspace.yml:59-60` deliberately omits `'**/.next/**'` from
`hasher.ignorePatterns`, so a bare `ts/**/*` would hash the whole built `.next`
tree — output this gate never scans — and re-key on every console build. The
negated entry is what keeps the broad form affordable, and § 13 **M4b** measures
that Moon 2.5.3 honours a negated **input** glob — by task hash, with a positive
control, and with a third arm confirming the same `.next` file DOES re-key the
task once the negation is removed. (§ 13 M4 measures the negated **output** glob
§ 8.1 needs; it is a separate mechanism and was never evidence for this one.)
`ts/node_modules/**` needs no entry: `hasher.ignorePatterns` already covers it.

**Revision 2 declared `ts/apps/**/*` and `ts/packages/**/*` instead, and claimed
the divergence from the corpus "is one-directional: the gate may run when
nothing it scans changed, and never the reverse". That claim was FALSE.** Twelve
tracked, scanned files sat outside those two globs: `ts/eslint.config.js`,
`ts/package.json`, `ts/moon.yml`, `ts/pnpm-workspace.yaml`,
`ts/tsconfig.base.json`, `ts/.npmrc`, `ts/.prettierignore`, `ts/.prettierrc.js`,
`ts/commitlint.config.cjs`, `ts/scripts/check-config-only.mjs` and both
`ts/tooling/*.mjs`. Concretely: a PR adding `ts/scripts/build-env.mjs` that read
`NEXT_PUBLIC_API_URL` would never have scheduled the gate and would have merged
green. Widening to `ts/**/*` closes it — measured, zero scanned files now fall
outside the declared inputs.

What remains is the divergence in the harmless direction only: the inputs cover
**untracked** files under `ts/`, the scan does not. So the gate may run when
nothing it scans changed. That, and only that, is one-directional.

**Residual, stated plainly.** `repo:input-liveness` catches a **dead** glob. It
does not catch a **too-narrow** one. A future narrowing of this gate's `inputs`
would switch it off, and only § 6.1's corpus floor would notice, and only for the
`ts/` rename case. The alternative — an unconditional `ci.yml` step — closes that
residual but loses local coverage. The trade was made deliberately.

**Three further scan residuals**, recorded in the task comment and the README as
every comparable gate does:

1. The ban is scoped to `ts/`. A `NEXT_PUBLIC_` in `rs/Dockerfile`, `ops/`, or a
   Helm template is equally harmful and unscanned. That scope is a decision, not
   an omission: `ts/` is where a Next build reads the prefix from.
2. A split literal (`'NEXT_' + 'PUBLIC_'`) evades any text scan.
3. The `**/*.md` exclusion is safe only while no app compiles Markdown. An MDX
   app would make it a bypass.

### 6.6 What obligation 5 actually costs

`NEXT_PUBLIC_FREE_SH_CALL_SITES` is **not** a new table entry. Measured:
`check_self_invocation` (`ci_targets.py:1516-1519`) takes seven **required**
positional parameters, and there are **47 call sites** — one production call at
`:3147`, one f-string field access at `:2357`, and 45 self-test assertions. Every
one gains an argument.

Adding an eighth haystack also means: a new required parameter, a new haystack
block (mirroring `:1607-1615`), an entry in the required-parameter loop
(`:2605-2613`), `main()` disk read and wiring (`:3123-3125`, `:3147-3150`),
three docstring counts (`:1523`, `:1548`, `:1560`), and a self-test battery.

**The battery is four fixtures, not three** (`:2782-2835`): the per-line deletion
loop, a contamination fixture, a commented-out fixture, and a **neutered-guard**
fixture. The fourth reproduces a review-caught bypass — rewriting the control's
comparison to something always-false leaves every other pinned line
byte-identical, so the control reports "passed" unconditionally. This gate's
control has such a comparison, so it needs the fourth fixture.

Two things this measurement makes easier than feared. There is **no argparse and
no `--help` text** in the file, so nothing to update there. And
`check_self_scheduled_coverage` (`:1736-1775`) derives the "must be registered"
set from Moon's own resolved script text, so a missing `SELF_SCHEDULED_GATES`
entry reds automatically — `repo:ruff-ci` is the namesake incident that function
exists to prevent (`:627-638`).

`repo:ruff-ci` (SMA-539) and `repo:workflow-credentials` (SMA-593) were each
their own Linear issue for this reason. § 15 puts the scope question explicitly.

## 7. The eslint boundary preset

`@paigasus/next-config/eslint` exports a flat-config array. It uses the core
`no-restricted-imports` rule, scoped by `files:` globs. It adds no dependency.

Type imports are banned alongside value imports. A `import type` of
`@paigasus/sdk` from `@paigasus/ui` still couples the packages, so the core
rule's behaviour is what is wanted and `@typescript-eslint/no-restricted-imports`
(whose only added feature here is `allowTypeImports`) is not needed.

| Scope | Denied | Reason |
|---|---|---|
| `packages/paigasus-ui/**` | `next`, every `@paigasus/*` | § 6 rule 1: plain React, testable in jsdom |
| `packages/paigasus-sdk/**` | every `@paigasus/*` except `@paigasus/proto`; `react`, `react-dom`, `next` | server-only, `sdk → proto` |
| `packages/paigasus-app-shell/**` | `@paigasus/sdk`, `@paigasus/auth/server` | the shell is client-reachable |
| `apps/**` | `@paigasus/proto` | apps reach the contract through `sdk` |

### 7.1 Negations must be doubled; positives are belt-and-braces

Every group carries both the bare and the `/**` form, with negations doubled:

```js
group: ['@paigasus/*', '@paigasus/*/**', '!@paigasus/proto', '!@paigasus/proto/**']
```

**Revision 2 justified this with a claim that is now DISPROVEN.** It argued that
`no-restricted-imports` matches `patterns[].group` with gitignore-style globs
where `*` does not cross `/`, so `@paigasus/*` would match `@paigasus/sdk` but
not `@paigasus/sdk/client`, and the `ui` and `apps` rows would silently permit
every subpath import they exist to ban.

Measured against the `ignore` package directly, and by dropping the `/**`
variants from the `ui` and `apps` groups and re-running the suite: `@paigasus/*`
**alone already matches** `@paigasus/sdk/client` and `@paigasus/proto/gen/iam`.
Both mutated runs stayed green. `ignore`'s directory-boundary semantics recurse
into a matched prefix, unlike a plain glob.

**What is load-bearing is the doubled NEGATION**, and that was measured too:
dropping `!@paigasus/proto/**` from the `sdk` group makes the "sdk may import a
proto subpath" row fail, which would ban a legitimate import.

The `/**` positives are kept as belt-and-braces — harmless, and they document
the intent — but nobody should add a new group believing the bare form is
insufficient.

§ 9.3 asserts a **subpath** import is reported for every scope, not only a bare
specifier. That assertion is still worth having: it pins the behaviour against a
future `ignore` version changing its recursion rule.

### 7.2 Package existence, corrected

`@paigasus/sdk` exists today as a source-only scaffold
(`ts/packages/paigasus-sdk/package.json`). Revision 1 wrongly said it did not, so
its boundary rule can be validated against a real package now.

`@paigasus/app-shell` and `@paigasus/auth` do not exist. Their entries are
written now and are inert until SMA-506 and SMA-508 land.

**Inert globs need a liveness check.** If SMA-506 lands the package under a
different directory name, the rule silently never applies, and every § 9.3 test
still passes because those use synthetic paths.

A stated-reason check alone does **not** close this, and revision 2 wrongly
claimed it did. A reason string is never re-validated, so once
`packages/paigasus-app-shell` is permanently absent the stale reason keeps the
test green forever — exactly the outcome the check was supposed to prevent.

The test therefore also maps **package name to directory**: it reads every
`ts/packages/*/package.json` and asserts that no package declaring the
corresponding `@paigasus/<x>` name exists under any other directory. While the
package does not exist at all, the stated reason carries it. The moment it lands
under any directory name, the check reds unless the boundary scope's directory
matches. The expected package name is derived from the scope path rather than
held in a second hand-maintained list, because two lists drift.

### 7.3 Deviation from the § 6 diagram

The design diagram reads `apps → app-shell → { ui, auth/client }`. This preset
allows an app to import `@paigasus/ui` directly.

Forcing every component through an app-shell re-export barrel buys no safety and
costs a barrel. The rules that carry real safety are the four above.

### 7.4 What eslint cannot do

The client-versus-server boundary is a `'use client'` property, not an
import-path property. `import 'server-only'` at the `sdk` and `auth/server` entry
points is what enforces it, and those packages own that line when they land. This
issue puts `server-only` on its own `/runtime` entry (§ 5) and ships the eslint
half for the rest.

## 8. Moon and console changes

### 8.1 `.next/cache` exclusion (AC4)

`paigasus-console-ts:build` becomes
`outputs: ['.next/**/*', '!.next/cache/**/*']`.

Two things are measured in § 13 M4 before this is relied on. Whether Moon 2.5.3
honours a negated output glob. And whether the move from `outputs: ['.next']` (a
directory) to a glob list changes which files are captured — `*` does not match a
leading dot in Moon's matcher, as `moon.yml:802-806` records for
`.github/workflows/.*.y*ml`, and `.next` contains dot-prefixed entries. The
fallback, enumerating the kept subpaths, inherits the same dotfile question.

### 8.2 The console build's inputs

Two holes, one pre-existing.

**The new one.** `next.config.ts` now imports the factory, so the build reads
`@paigasus/next-config` sources. `/ts/packages/paigasus-next-config/src/**/*`
joins the task's `inputs`. Without it, editing the factory serves a cached build —
the staleness class SMA-519 paid for.

**The pre-existing one, larger.** The task declares
`inputs: ['@group(sources)', …]` (`ts/apps/paigasus-console/moon.yml:10`). For an
`application`-layer project, `sources` resolves from
`.moon/tasks/typescript-project.yml:18-19` to `src/**/*`. **The console has no
`src/` directory** — its pages are `app/layout.tsx` and `app/page.tsx`. So
editing a page does not re-key the console build today. `ts/moon.yml`'s wider
`sources` group belongs to the `ts` configuration-layer project, not to the app.

That is tolerable while the build asserts nothing. Once § 8.3 makes this task the
sole proof of AC1, an app-code change would replay a cached PASS and the
standalone check would never run. `'app/**/*'` therefore joins the inputs in the
same edit. `repo:next-env-drift` already works around the same hole by listing
`ts/apps/paigasus-console/app/**/*` in its own inputs (`moon.yml:152`).

`ts/pnpm-lock.yaml` is deliberately **not** added.
`repo:next-env-drift` keys on the lockfile precisely because the build does not,
and its comment records why. Changing that premise is a separate decision.

### 8.3 The standalone assertion (AC1)

`paigasus-console-ts:build` becomes a `script:` block opening with
`set -euo pipefail` — Moon does not enable errexit for `script:` blocks
(`moon.yml:70-72`, `:601-605`), so without it a failed `next build` followed by a
satisfied file check exits 0. That is a false green on the issue's headline AC.

The block runs `next build`, then fails with a named message when the standalone
entry point is absent.

The asserted path is a **literal**, in two places: the `script:` block in
`ts/apps/paigasus-console/moon.yml` and `tests/standalone-runtime.test.ts:13`.
It is not derived from § 4.3's `outputFileTracingRoot` at check time. What § 13
M2 gives is the measured VALUE that literal was written from, not a derivation.
That is acceptable because the literal fails LOUDLY if it drifts — a moved entry
point makes the build assertion red and the smoke unable to start the server —
and never silently, which is the property that matters. A future
`outputFileTracingRoot` change means editing both sites.

A unit test on the returned config object proves the factory returns
`output: 'standalone'`. It does not prove Next honoured it. A Next version that
renames or ignores the key would pass a unit test and ship a broken image.

Deleting `.next` before the build was considered and rejected: `next build`
already clears its dist directory except `cache`, and `set -euo pipefail` aborts
before the assertion on a failed build, so the delete would only cost the
incremental cache. § 13 M2 confirms the clearing behaviour while it measures the
path.

### 8.4 The standalone runtime smoke (AC1, second half)

The artifact check proves the file exists. It does not prove the server reads
config at runtime, which is what AC1 actually claims. The two failure modes most
likely to ship — Next inlining a value, and the standalone bundle omitting the
transpiled package — are invisible to both the artifact check and the unit tests.

So the console gains a server component on `/` that renders its zone id and zone
map from `getRuntimeConfig()`, and a smoke step starts the built standalone
server twice with **different** `PAIGASUS_ZONES` values and asserts the responses
differ.

This also resolves who first consumes `getPublicConfig()`. Without it, the § 9.2
sanitizer tests exercise a function with no production call site.

### 8.5 Affected-graph impact

**No existing `ci/affected-graph/run.sh` case changes.** Every `run_case`,
`run_task_case` and `run_task_case_ci` (`run.sh:258-353`) touches a file under
`rs/` or `contracts/`, and `--downstream deep` from those never reaches a new
`ts/packages/*`.

This holds only while `@paigasus/next-config` declares no workspace dependency on
a kernel-derived package: `kernel->bindings` already lists `paigasus-kernel-ts`
under strict equality.

The § 7.1 landmine in the design document — strict equality redding when
`sdk → proto` lands — is real and belongs to SMA-508.

### 8.6 Dev ergonomics

The console gets a `.env.local.example` documenting both variables. `next dev`
reads `.env.local`, which stays untracked.

### 8.7 The tsconfig preset's one adoption cost

`@paigasus/next-config/tsconfig-app` (§ 4, `tsconfig.app.json`) gives every
console zone app one preset: the shared `lib`, `jsx`, `plugins`, and the rest of
the workspace base, all from a single `extends`. Every future console zone app is
meant to adopt it the same way.

An app that adopts the preset AND runs vitest (§ 8.4's runtime smoke is the first
case) also needs its own `tests/tsconfig.json`, extending
`ts/tsconfig.base.json` directly by relative path — the same one-line-extends
pattern every non-Next package in the workspace already uses. MEASURED, not
inferred: renaming `paigasus-console`'s `tests/tsconfig.json` away and
re-running `vitest run` reproduces the failure below on demand.

The reason is a resolver mismatch, not a config mistake. `tsconfig.app.json`
extends the workspace base by relative path (`../../tsconfig.base.json`), which
is only correct because pnpm symlinks the package into `node_modules` and the
resolver is expected to follow that symlink to its real path before applying the
`../../`. `tsc` does exactly that — `tsc -p tsconfig.json --noEmit` exits 0.
Vitest's oxc-based transform does not: it applies the `../../` relative to the
symlink's own location inside `node_modules`, misses, and fails hard rather than
degrading, with:

```
[TSCONFIG_ERROR] Failed to load tsconfig for '<file>': Tsconfig not found
```

That text is what a future author will see in a stack trace; it is named here so
it is greppable back to this section.

**Rejected alternative: inline the base options into `tsconfig.app.json` and drop
the `extends`.** This would remove the symlink hazard entirely, and was the
reviewer's first suggestion on the PR that found this. It was rejected anyway: it
would duplicate roughly fourteen compiler options across `tsconfig.base.json` and
`tsconfig.app.json`, two files that must then agree, with nothing gating that
agreement — an ungated drift surface, which this repo treats as a serious failure
mode (see the version-lockstep and cargo-config-input entries in the root
CLAUDE.md for the same reasoning applied elsewhere). The chosen cost — one small
per-app file, and a failure mode that is loud and documented rather than a
config pair that can silently drift apart — was judged cheaper.

## 9. Testing

### 9.1 Factory

- `output` is `'standalone'`.
- `extend` cannot override `output`.
- An `extend.env` key throws.
- The compiled zone and base path reach `env`.
- `outputFileTracingRoot` is set.
- `assetPrefix` is absent unless the caller supplies it.

### 9.2 Runtime

- A valid env parses.
- An invalid env throws, and the message names the variable.
- No error message contains a variable's value.
- The second call returns the memoized value after `process.env` is mutated.
  This proves the memo rather than assuming it.
- `NEXT_PHASE=phase-production-build` throws.
- A zone mismatch throws, naming both values.
- A base-path mismatch throws, naming both values.
- An **absent** `PAIGASUS_COMPILED_ZONE` throws — the fail-closed case.
- `/iam`, `/iam/` and `iam` are equivalent; the root zone (`''`) is accepted.
- A non-path value in the `PAIGASUS_ZONES` JSON is rejected at parse time.
- An `extraShape` declaring `PAIGASUS_ZONE` throws.
- An extra shape carrying a secret does not appear in `getPublicConfig()`.
- `publicConfigSchema`'s key list is pinned by strict equality.

### 9.3 Boundaries

ESLint's Node API runs the real exported preset over `lintText`, with synthetic
file paths. Synthetic paths mean the `app-shell` and `auth` entries are tested
now, before those packages exist.

- Each denied import reports `no-restricted-imports`.
- Each denied **subpath** import reports it too (§ 7.1).
- Each allowed import reports nothing. A rule that reports everything is not a
  boundary rule.
- Each `files:` glob matches an existing package directory or carries a stated
  forward-guard reason (§ 7.2).

### 9.4 Guard the guard

One test asserts `ts/eslint.config.js` actually consumes the preset — and that it
appears in the **exported array**, not merely in an `import` statement, because a
dead import satisfies a text scan.

Without this, deleting the import leaves every boundary test green. A fixture
table exercises a verdict function; it never exercises the invocation.

**That test is unreachable unless its task keys on the file.**
`paigasus-next-config-ts:test` inherits
`inputs: ['@group(sources)', '@group(tests)', 'package.json', '/ts/pnpm-lock.yaml']`
from `.moon/tasks/typescript-project.yml:46-52`. `ts/eslint.config.js` is not
among them, so Moon would replay a cached PASS on exactly the PR that breaks it.
The package's `moon.yml` therefore adds `'/ts/eslint.config.js'` to `test.inputs`.
This is a requirement of the design, not an implementation detail.

### 9.5 Why no fixture directory

AC3 says "a test fixture". A real fixture directory of deliberately-wrong imports
would have to be excluded from the workspace `ts:lint` and `ts:fmt` runs. An
excluded fixture is exactly the kind of thing that goes inert unnoticed.

In-memory `lintText` needs no exclusion and tests the same shipped config.

## 10. New dependencies

`zod` joins the pnpm catalog at `^4.5.4`, published 2026-08-29 — clear of
pnpm 11's 24-hour `minimumReleaseAge`. Zod **4** semantics are assumed throughout:
unknown-key stripping by default, `z.record(keyType, valueType)` taking two
arguments, and `error.issues[]` as the error surface (§ 5.5).

`server-only` joins the catalog for § 5.

## 11. Out of scope

Each item is owned elsewhere.

- Renaming `paigasus-console` to `iam-console` — SMA-511.
- `import 'server-only'` on `sdk` and `auth/server` — SMA-506, SMA-508.
- Capability keys in the public slice — ADR-0020.
- The `ci/affected-graph` strict-equality re-baseline — SMA-508.
- Containerization and Helm charts — design § 7.1.
- Inline-`<script>` transport of the public slice, and its escaping rule (§ 5.4).
- `ts/apps/paigasus-docs`. It is not a Next app today. It adopts
  `createNextConfig` if and when it becomes one, and § 6.2's app check would
  then require it.

## 12. Acceptance criteria mapping

| AC | Where it is satisfied | Proof |
|---|---|---|
| 1. An app builds to a standalone server reading config at runtime | § 4, § 5, § 8.3, § 8.4 | real `next build` + a two-value runtime smoke |
| 2. New CI gate: `NEXT_PUBLIC_` is banned | § 6 | self-test + negative control over git fixtures |
| 3. The boundary rule fails a deliberately-wrong import | § 7, § 9.3 | real preset via ESLint's Node API |
| 4. Moon `build` outputs exclude `.next/cache` | § 8.1 | § 13 M4 |

## 13. Measurements — TAKEN

The first five were measured on 2026-09-08 against Next 16.3.4 with Turbopack
(the default `next build` bundler in Next 16) and Moon 2.5.3; M6/M6b/M6c were
measured on 2026-09-09 against the same versions plus zod 4.5.4. Full commands
and literal outputs:
`docs/superpowers/specs/2026-09-08-sma-502-measurements.md`.
**Re-take them on a Next, Moon or zod bump.**

**M1 — does the build-phase throw actually fire, including in prerender
workers? YES.** A page calling a `NEXT_PHASE` guard at module scope fails
`next build` at rc 1, and the throw appears twice in the log. So workers do
inherit the assignment, and § 5.3's premise holds end to end rather than only in
the main process. The `connection()` fallback is not needed.

**M2 — the real standalone entry point is
`.next/standalone/apps/paigasus-console/server.js`**, with
`outputFileTracingRoot` pinned to `ts/`. This confirms § 4.3: the bare
`.next/standalone/server.js` the design first assumed would have been wrong.
`next build` also clears `.next` except `cache`, so § 8.3's assertion cannot
pass on a stale artifact.

**M3 — `basePath` and `assetPrefix` do NOT compose.** See § 4.4; the double-prefix
hazard is disproven.

**M4 — Moon 2.5.3 honours negated `outputs` globs**, measured against the cached
output archive: `.next/BUILD_ID` is captured and `.next/cache/**` is not. So
§ 8.1's `!.next/cache/**/*` stands as written and its fallback is not needed.

**M4 originally claimed the `inputs` half too, and had not measured it** — it
changed only `outputs` and inspected only the output archive. An output negation
controls what the cache ARCHIVES; an input negation controls what the cache KEYS
ON, and Moon implements those separately, so the one was never evidence for the
other. **M4b** (added at CodeRabbit round 1) supplies the missing measurement:
three arms comparing `repo:next-public-free`'s task hash. An excluded `.next`
file leaves it unchanged (`cae69cf0` -> `cae69cf0`); a new tracked `ts/` file
changes it, which is the positive control proving the probe is live
(`cae69cf0` -> `8867aa6f`); and with the negation line deleted from `moon.yml`
the same excluded file DOES change it (`acfd6118` -> `59cd599f`), so the
negation is causally responsible rather than the path being unhashed anyway.
So § 6.5's input list now stands on its own measurement, and its fallback is
not needed either.

**M5 — both `next.config.ts` and `ts/eslint.config.js` can import TypeScript
source from the workspace package**, with one packaging condition: the consuming
`package.json` must declare the dependency itself. `ts/eslint.config.js` is owned
by `ts/package.json`, so that root manifest needs the devDependency;
`workspace:*` resolution does not fall through to a sibling app's
`node_modules`.

The `.mjs` fallback is therefore **not forced**. The eslint preset still ships as
`.mjs` by choice — it is configuration data and TypeScript buys little there —
but it does so with no `.d.mts` and no drift surface. Had the fallback been
forced, the declaration file would have had to be **generated** with
`tsc --emitDeclarationOnly` and drift-checked rather than hand-written: a
hand-written declaration is read by `tsc` instead of the implementation, the same
shape as `repo:pyo3-stub-drift` (SMA-600) and the still-open SMA-535.

**M6 — does `server-only` make a middleware import a build error? NO.** A
throwaway `middleware.ts` importing `@paigasus/next-config/runtime` builds at
**exit 0**, and the module lands in `.next/server/edge/chunks/`. `server-only`
throws only when the `react-server` condition is ABSENT, and Next sets it for the
middleware layer, so the import resolves to `empty.js` there. `server-only` is a
**client-bundle** guard. Revision 2's claim to the contrary is corrected in § 5.

**M6b — `process.env.NEXT_RUNTIME` is a compile-time define, so the edge guard
resolves statically.** `next/dist/build/define-env.js:80` sets it to the literal
`'edge'` for the edge compilation. Measured on the rebuilt probe: the edge chunk
carries an **unconditional throw**, and the node chunk carries **no** guard code
(`grep -c` returns 0). This is what closes the edge door in § 5.

**M6c — only one zod `.refine()` message form leaks the input.** On zod 4.5.4,
the zod-3 `(v) => ({ message })` second-argument form is ignored and yields the
generic `Invalid input`; `{ error: (iss) => \`${iss.input} …\` }` renders the
input verbatim. § 5.5's `describeIssues` filter and its test are written against
the second form, because a test using the first would assert nothing.

## 14. What changed in revision 2

The adversarial challenge returned NEEDS REWORK. Folded in: the fail-closed and
canonical-form fixes to the cross-check (§ 4.2); `outputFileTracingRoot` (§ 4.3);
the `assetPrefix` default removed as wrongly justified (§ 4.4); `server-only` and
the edge-runtime constraint (§ 5); the key-collision rule and the record-value
limit on zod stripping (§ 5.1, § 5.2, § 5.4); a named transport (§ 5.4); the
corpus floor, git-initialised fixtures, exit-code contract and app-uses-factory
check (§ 6.1–§ 6.3); corrected obligation-7 justification and the true cost of
obligation 5 (§ 6.4, § 6.6); inputs narrowed off `.next` (§ 6.5); three scan
residuals (§ 6.5); subpath glob forms and the inert-glob liveness check (§ 7.1,
§ 7.2); `set -euo pipefail` on the build script and the reachable guard-the-guard
inputs (§ 8.3, § 9.4); the console's missing `app/**/*` input (§ 8.2); the
runtime smoke (§ 8.4); and the generated-not-hand-written `.d.mts` fallback
(§ 13 M5).

Corrected from the challenge itself: the `NEXT_PHASE` premise is partly measured,
not wholly unverified — `next/dist/build/index.js:1212` sets it and nothing under
`next/dist/server/` does. The open half is worker inheritance, and § 13 M1 covers
it.

Rejected: deleting `.next` before each build (§ 8.3 states why).

## 15. The scope decision, settled

§ 6.6 measures obligation 5 as surgery on `check_self_invocation`, not a table
entry. `repo:ruff-ci` and `repo:workflow-credentials` were each their own issue
on that basis. AC2 nonetheless asks for the gate in SMA-502.

**Decision (2026-09-08): the gate stays in SMA-502, with all seven obligations,
the script pin included.** Two alternatives were put and rejected: splitting the
gate into a follow-up issue, which leaves the single-image rule unguarded until
that issue lands; and keeping the gate but dropping the script pin, which leaves
its own production call site deletable while green — the failure § 9.4 argues
against elsewhere.

So the plan carries the `ci_targets.py` work explicitly: a new required
positional parameter on `check_self_invocation`, a new haystack block, `main()`
wiring, help text, a self-test battery, and an updated argument list at every
existing call site.
