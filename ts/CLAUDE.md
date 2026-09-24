# paigasus-core — `ts/`

<!-- Project memory. Claude Code loads this file only when it reads a file in
this directory, so it costs nothing in a session that stays out of ts/.
The root CLAUDE.md holds the repo-wide rules and the two gate-checked blocks. -->

## Build and module resolution

- **vitest 5 resolves a Node-environment test's imports through `ssr.resolve.conditions`, NOT the
  top-level `resolve.conditions`** (MEASURED on 5.0.0, SMA-502). Setting only the top-level key has
  no effect on an `environment: 'node'` project, so a package whose source depends on a resolution
  condition fails at import with the top-level block present and apparently correct. Set BOTH —
  `ts/packages/paigasus-next-config/vitest.config.ts` is the worked example. vitest moved 4.1.11 ->
  5.0.0 in `29c03977`, so nothing in the repo had exercised this before.
  The case that surfaced it: `@paigasus/next-config/runtime` opens with `import 'server-only'`,
  whose exports map is `{ "react-server": "./empty.js", "default": "./index.js" }` — and `index.js`
  is nothing but an unconditional `throw`. Without the `react-server` condition every test in the
  package dies at import. **The fix is the condition, never deleting the import**: that line is the
  structural guard keeping the module out of client bundles, and removing it greens the suite while
  destroying the protection.
  List the additive module-resolution defaults alongside it — `['react-server', 'node', 'import',
  'default']`, not a bare `['react-server']`. A single-entry list drops `import`/`default` and
  breaks source-exports `.ts` resolution for every `@paigasus/*` package, which is the same trap
  `ts/packages/paigasus-kernel/vitest.config.ts` already records for its browser project.
  Note `server-only` guards the CLIENT bundle only. Next sets the `react-server` condition for the
  middleware layer too, so it resolves to `empty.js` there and is a no-op — an explicit
  `process.env.NEXT_RUNTIME === 'edge'` check is what covers the edge runtime (measured: the guard
  compiles to an unconditional throw in the edge chunk and is absent from the node chunk).
- Tailwind v4's automatic scan root is the **current working directory**, and Moon runs `next
  build` from the app's own directory — not the repo root. So every consumer of `@paigasus/ui`
  needs its own `@source` line covering `ts/packages/paigasus-ui/src`; forgetting it drops the
  package's classes silently, and only in a PRODUCTION build (`ts/apps/iam-console/app/globals.css:23`
  is the first copy). Next 16.3.4 builds with Turbopack and writes CSS to
  `.next/static/chunks/`, not `.next/static/css/`, and there is **no**
  `.next/app-build-manifest.json` at all — so `ci/tailwind-source/run.mjs` walks `.next/static`
  recursively instead of reading a manifest, and `iam-console-ts:build` removes
  `.next/static` before every build so a stale chunk from an earlier build cannot satisfy that
  walk. The guard script lives at `ci/tailwind-source/` and must **never** move under
  any `ts/apps/*` directory, because each one is Tailwind's scan root for its own app and a
  script holding the sentinel literal (`--paigasus-ui-source-probe`) would make Tailwind generate
  the very utility it asserts on — and the guard's assertion-3 scan is a **full walk of the named
  app's own directory** (parameterized by `--app`), not an allowlist, because the old
  `['app'] + four config files` list missed `moon.yml`, `next-env.d.ts` and `.prettierignore`, all
  of which Tailwind reads.
  `iam-console-ts:build` also uses `options.merge: replace`, so
  it inherits nothing from `.moon/tasks/typescript-project.yml` and lists `/ts/pnpm-lock.yaml`
  **and `/ts/tsconfig.base.json`** by hand in its own `inputs` (`test` replaces too and needs
  both; `typecheck` merges and inherits them). `repo:affected-smoke`'s **two** `ui->console`
  cases (`ci/affected-graph/run.sh`) are the only control on the input list that makes the guard
  real — they assert a `@paigasus/ui` source edit selects both `iam-console-ts:build` and
  `iam-console-ts:test`; without them, an input dropped from either task's `inputs` serves a
  cached `.next` and the guard passes against stale CSS. There are two because one anchors on
  `src/styles/tokens.css` and one on `src/components/table.tsx`: a single anchor leaves the
  console's `src/**/*` input narrowable to the other subtree while the case stays green. Note a
  residual the guard does NOT close: `rm -rf .next/static` lives inside the build task's own
  `script:`, so a Moon **cache hit** hydrates `.next` without running it and the walk can satisfy
  both sentinels from a stale chunk (`ci/tailwind-source/README.md`, Limitations). And **the
  shadcn CLI is unusable in this package** — measured, it adds an unrelated npm package literally
  named `cn` to the manifest and the lockfile of a public repository, on top of writing to a
  literal `./@/components/` directory; every component here is hand-written. Read
  `ts/packages/paigasus-ui/README.md`'s "The `shadcn` CLI is not usable in this package" before
  running it. (SMA-503)
  Since SMA-512 the guard is **per app**: `ci/tailwind-source/run.mjs --app <dir>`, invoked by each
  app's own `test` task, and a BARE run now exits 2 rather than silently checking `iam-console`.
  `TAILWIND_GUARD_INVOCATIONS` holds **two** apps today (`iam-console`, `gateway-console` — SMA-512
  pull request 3), each invoking the guard for itself in its own `test` task; a third app repeats
  the same shape. That registry, in `ci/affected-graph/ci_targets.py`, fails `repo:affected-smoke` if
  a `ts/apps/*` directory with a `package.json` does not invoke all three modes for itself, in its
  Moon project's resolved `test` script — the fix wave closed three ways to defeat this: an entry
  no longer stores hand-copied lines (they are derived from the app name, so an entry cannot name
  another app's `--app` directory), the check matches moon's resolved script rather than the raw
  `moon.yml` text (a line parked in another task, or one that never runs, no longer counts), and a
  `package.json`-bearing directory with no matching Moon project is reported rather than skipped.
  `repo:next-env-drift` and the Next ESLint blocks are app-agnostic too: the first discovers
  `ts/apps/*/next.config.*` and asserts every `ts/apps/*` directory that has a `package.json` is in
  the discovered set, and `ts/eslint.config.js` derives one block per app directory. The next-env
  gate still has **no negative control**. Its `deps` names one build per app by hand and nothing
  asserts the list is complete, so a new app must add its own `<app>-ts:build` edge or `next
  typegen` races that app's `.next`.
- `repo:next-public-free`'s `APP_CONFIG_FLOOR` is **2** since the second console zone landed,
  pinned as a whole line (`"APP_CONFIG_FLOOR=2"`) in `ci/affected-graph/ci_targets.py:1251`, so
  the constant in `ci/next-public/run.sh` and its pin move together or the gate reds. It is a
  **collapse detector**, not a per-app assertion. MEASURED reason: `APP_CONFIG_GLOB='ts/apps/*/
  next.config.[tjmc][sj]*'` matches **four** tracked paths today, not two — the two real apps'
  configs plus `ts/apps/iam-console/tests/fixtures/{client,server}-imports-sdk/next.config.ts` —
  because a git pathspec's `*` spans `/`, the same pathspec trap this file already records for the
  ruff gate's `ci/**/*.py` corpus. The gate's own `app_configs()` then filters with
  `grep -E '/next\.config\.(ts|js|mjs|cjs)$'`, which does **not** exclude those fixtures, since
  they end in `next.config.ts` too. So deleting one app's `next.config.ts` leaves 3 ≥ 2 and the
  gate stays **green**. What actually holds a specific app's config in place is that app's own
  build and `ci/next-env/run.sh`'s per-app discovery — not this floor.
- **Turbopack (Next 16.3.4) does NOT resolve a `.js` relative specifier to a `.ts` file** (MEASURED,
  SMA-510): `import { x } from './a.js'` with only `a.ts` on disk fails `next build` with `Module not
  found`, in app code and in a workspace package's source alike. A clause-level `import type … from
  './a.js'` is erased first and builds (measured). `import { type A } from './a.js'` is not erased
  under `verbatimModuleSyntax` (reasoned from the flag's rules, not separately measured). So every
  file a Next app compiles uses EXTENSIONLESS relative value imports. SMA-511 made that true for every
  package the IAM console compiles: the `src/` of `@paigasus/auth`, `@paigasus/sdk`,
  `@paigasus/discovery` and the hand-written `@paigasus/proto` files, and BOTH buf templates
  (`contracts/buf.gen.yaml`, `contracts/buf.gen.googleapis.yaml`) no longer pass
  `import_extension=.js`, so the generated protobuf-es code is extensionless too. Two controls hold
  it. The ESLint rule `paigasus/no-js-relative-specifier` reports an `import`, `export … from` or
  `import()` whose `./`/`../` specifier ends in `.js`, under `packages/*/src/**`, test files
  excluded. It ships as `sourceRules` from `@paigasus/next-config/eslint`, NOT inside
  `boundaryRules` (a `packages/*/src` scope there fails the reverse liveness loop), and
  `ts/eslint.config.js` spreads it, which a test pins. It is a rule with its OWN name on purpose: in
  flat config a second `no-restricted-imports` block that matches the same files REPLACES the first
  and switches the boundary rules off without a word. ESLint ignores `**/generated/**`, so a
  `@paigasus/proto` test asserts the same thing for `src/generated/`. Test files keep their `.js`
  imports (vitest resolves both). The two plain-Node loaders that run package source
  (`paigasus-auth/tests/fixtures/ts-esm-loader.mjs`,
  `paigasus-discovery/tests/containers/support/ts-esm-loader.mjs`) retry an extensionless specifier
  as `.ts`, then `/index.ts`, because plain Node does not probe extensions. Vite, vitest, tsc and
  Playwright accept both forms, so only a Next build or these two controls notices a regression.
- **`@paigasus/kernel` resolves to its WASM entry, on the server too** (SMA-634, ADR-0022). A Next
  server build cannot load the napi binding from an in-monorepo `file:` or `link:` dependency:
  pnpm installs a `file:` target once, at install time, by the package's `files` allowlist, and
  Turbopack refuses a `link:` symlink whose target is outside its root. So the kernel's `.` export
  is the wasm entry under every condition, and the napi entry is `@paigasus/kernel/napi`, which only
  the kernel's own tests name. The published npm model of `@paigasus/node-bindings` (a loader-only
  package plus seven per-platform packages) is correct and unchanged.
  `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm` and its four glue files are **committed**,
  because pnpm links the crate before any build task runs. `moon run
  paigasus-kernel-ts:generate-wasm` is their only writer: run it after a Rust kernel or wasm-binding
  edit, and commit all five. `paigasus-kernel-ts:test` holds them to the source with four checks —
  the committed glue equals a fresh build, the binary's import and export lists equal a fresh
  build's, the committed pair replays all five parity corpora, and (in the console vitest
  `setupFiles`) the pnpm-installed copy equals the committed files. **No check compares the binary
  bytes**: they differ on macOS, Linux arm64 and Linux amd64, while the glue and the interface do
  not. After a `git checkout`, a rebase or a branch switch that replaces those files, run `rm -rf
  ts/node_modules && pnpm -C ts install`: pnpm hard-links a `file:` dependency and does not repair a
  broken link, not even with `--force`. Only ONE host regenerates the artifacts; a second host makes
  different bytes and a diff that says nothing. A conflict in the five files is resolved by taking
  either side and running `generate-wasm` again.
  `@paigasus/console-core`'s `src/prn-tenancy.ts` is now an IAM tenancy adapter over the kernel, not
  an ADR-0005 exception: it keeps IAM's tenancy rule, the UUID guard for a URL segment and a length
  limit for the FFI boundary. `tests/unit/prn-tenancy-delegation.test.ts` mocks the kernel and fails
  if the file parses a PRN itself. A wasm that cannot load 500s EVERY `(console)` page, not only the
  PRN pages, because `src/index.ts` re-exports the module and `@paigasus/wasm` is side-effectful.
  Node prints an `ExperimentalWarning` for the wasm import in every console vitest run; it is
  harmless.
- The `ts` project's `sources` group names app code directories BY HAND (`apps/*/app/**/*`,
  `apps/*/lib/**/*`, `apps/*/proxy.ts`). `ts:lint` runs `eslint .` over the whole tree, but Moon
  re-runs it only for a file in its `sources` or `tests` group (or one of its config inputs), so a
  new top-level app directory needs a line in `sources`, or an edit to it serves a cached lint PASS.
  The same holds for a top-level app file such as `playwright.config.ts`.
- The console image rules live in two places already. Do not copy them here. The static-asset
  staging rule is at `docs/ops/RUNBOOK-containers.md:364-369`. The exec-form
  `ENTRYPOINT`/`HEALTHCHECK` rule (no `ARG`/`ENV` expansion) is at `rs/CLAUDE.md:207` and
  `docs/ops/RUNBOOK-containers.md:317-322`.
  The kind-only specs in `apps/iam-console/tests/cluster/` run only from `ci/kind/run.sh specs a|b|journeys`.

## Auth, sessions and the console packages

- **`@paigasus/auth` under a Next `basePath`** (MEASURED on Next 16.3.4, SMA-511 spec § 13 row 1).
  Next removes the basePath before app code sees a path, in three different places. In `proxy.ts`,
  `req.nextUrl.pathname` has no `/iam`, while `req.nextUrl.basePath` is `/iam` and `req.url` keeps
  it. In a route handler, `req.url` has no basePath AND carries the server's bind address
  (`http://0.0.0.0:<port>`); only its scheme follows `X-Forwarded-Proto`. A page
  `redirect('/auth/login')` gets the basePath added once, and `redirect('/iam/auth/login')` becomes
  `/iam/iam/auth/login`. So `authRoutePaths()` takes no argument and returns basePath-RELATIVE
  paths, `requireSession` redirects to the relative login path, `createAuthRouteHandler` rebuilds the
  URL from `AuthRuntime.publicOrigin` + basePath, and `handleCallback` builds openid-client's
  `currentUrl` from `runtime.redirectUri`. That last part broke `redirect_uri` equality even with NO
  basePath. A unit test sees none of this without `new NextRequest(url, { nextConfig: { basePath:
  '/iam' } })`, and the plain-Node auth e2e harness passes full paths, so the iam-console e2e tier
  (`iam-console-ts:test-e2e`, row R2) is the only end-to-end control.
- **Next gives a route handler and a page SEPARATE module graphs, so a module-level singleton is
  per-layer** (MEASURED, SMA-511). `getAuthRuntime`'s module-level cache produced two memory
  session stores, one per layer, and login looped forever: a page set a session in its store and
  redirected, the route handler checked a different, empty store, and sent the user back to log in
  again. The fix caches the runtime on `globalThis` under a `Symbol.for(...)` key instead, so both
  layers share one instance. No unit test can catch this: each layer's code is correct in
  isolation, and only a real Next build with both layers wired together — the iam-console e2e tier
  — reproduces the split.
- **`@paigasus/auth`'s `returnTo` loop guard is BYTE-EXACT.** It collapses dot segments and
  repeated slashes before comparing a path to the auth route prefix, but it does not decode
  percent-escapes or fold case: `/iam/%61uth/login` and `/iam/AUTH/login` are not refused. No loop
  exists today, because the route table serves neither spelling (both 404 before the guard would
  matter). This holds only as long as no proxy or router in front of the app decodes or case-folds
  a path before routing on it.
- **`@paigasus/console-core`** (`ts/packages/paigasus-console-core`, SMA-512 PR 2) is a source-only,
  private, server-only package holding the console composition both zones share: the IAM client
  factory, provisioning and the principal resolver, `mayI()`, `myScopes()`, `callIam`, the
  JSON-lines logger, discovery, the correlation helpers and the PRN readers. It is the ONE package
  allowed to import both `@paigasus/auth` and `@paigasus/sdk`: `@paigasus/auth` must not import
  `@paigasus/sdk`, and the sdk boundary block bans every other `@paigasus/*` import, so before this
  package only an app could depend on both. Its `createConsoleRuntime` factory must be called
  EXACTLY ONCE per app, at module scope: every accessor but `iamClientsForToken` and
  `iamClientsForAction` is a React `cache()` wrapper, and a second call makes a second memoization
  identity — a second `WhoAmI` (SMA-632; previously `Introspect`), a second `ListRoleGrants` walk,
  and up to 50 more tenancy reads per render. Outside a React server render `cache()` is a
  pass-through, so NO unit or integration test can catch a violation; only an e2e `WhoAmI` count
  can, and that lives in a later pull request. `iamClientsForAction` is deliberately not memoized: it returns a `relogin` failure
  rather than redirecting, so a Server Action can render an inline error. Its `testing/` subpath is
  OUTSIDE `src/` and carries no `server-only` guard, on purpose: vitest and Playwright harnesses
  import it outside a Next server. "Every file under `src/` imports `server-only`" has ONE exception:
  `src/global.d.ts` declares a type only and imports nothing, so it does not import `server-only`
  either — harmless, since a `.d.ts` emits no runtime code. What actually keeps a client bundle safe
  is the package's `exports` map, which exposes only `.` (`src/index.ts`) and `./testing`
  (`testing/index.ts`), so no deep import can reach an unguarded module, together with
  `src/index.ts`'s own `import 'server-only'`. The per-file rule is defence in depth on top of that,
  not the guard itself.
- **node-redis `socket.socketTimeout` is an IDLE timer, not a reply deadline** (read from
  `@redis/client` 6.2.1, SMA-648). Any read OR write on the socket resets it, so it fires on a
  quiet, healthy connection, and it bounds a hung command only while the socket is otherwise silent
  — under steady traffic a Redis that accepts commands and never replies is still unbounded (SMA-650
  added a per-operation deadline in `@paigasus/console-core`. SMA-651 added the same shape to
  `@paigasus/auth`'s session store, as the decorator in `src/adapters/operation-deadline.ts`. Its
  `close()` destroys the connection at once. node-redis's graceful `close()` waits for a reply that
  never comes, so the decorator does not use it). node-redis's DEFAULT reconnect strategy returns
  `false` for a `SocketTimeoutError`, so the first idle gap closed the console descriptor cache's
  client for the life of the process (`The client is closed`, nav degraded). A client that sets
  `socketTimeout` therefore needs `pingInterval` (at most half of `socketTimeout`) to keep an idle
  socket alive, and a `socket.reconnectStrategy` that returns a delay for EVERY cause.
  `createRedisDescriptorCache` asserts all six options; the Redis user needs `+ping`.
  `paigasus-console-core-ts:test-e2e` is the Docker-backed control, with no skip hatch, so a
  console-core source edit now needs Docker.
- **`forbidden()` needs `experimental.authInterrupts`, and a React `cache()` value does not reach
  `forbidden.tsx`** (MEASURED on Next 16.3.4, SMA-511). Without the flag, `forbidden()` throws
  instead of rendering the 403 boundary. The iam-console sets it through
  `createNextConfig({ extend: { experimental: { authInterrupts: true } } })`, and its vitest env needs
  `__NEXT_EXPERIMENTAL_AUTH_INTERRUPTS=true` or the call throws E488. A nested `forbidden.tsx` is a
  per-segment boundary: `(console)/forbidden.tsx` renders inside the `(console)` layout with a real
  HTTP 403. `forbidden()` takes no argument, and a `cache()` holder set before the call is EMPTY in
  the `forbidden.tsx` render, so the view cannot receive request data that way.
  The view gets the correlation id from a request header instead: `proxy.ts` mints it,
  `@paigasus/console-core`'s `iam-clients.ts` sends it to IAM as `paigasus-correlation-id`, IAM
  adopts it, and `forbidden.tsx` reads it with `headers()`. `@paigasus/console-core`'s
  `correlation.ts`'s `FORBIDDEN_VIEW_CORRELATION` records which of the two ships,
  and e2e row R4 fails if the view does the other. The flag is experimental: R4 asserts the real
  HTTP 403, so a Next upgrade that changes it reds CI.
- **`ts/apps/gateway-console`** (SMA-512 PR 3) is the second console zone: a Next.js 16 App Router
  app for the AI Gateway, mounted at `/gateway`, Moon id `gateway-console-ts`. Its `lib/config.ts`
  demands **both** an `iam` entry and a `gateway` entry in `PAIGASUS_SERVICES` — it refuses to
  parse a map missing either one — and declares no gateway-specific env key of its own; the
  gateway's address comes only through `PAIGASUS_SERVICES.gateway`, unlike IAM, which also carries
  its own `PAIGASUS_IAM_GRPC_URL`. The zone overview lives at **`(console)/overview/page.tsx`** →
  `/gateway/overview`, **not** at `/gateway/` — the public landing page already owns that path, and
  a second `page.tsx` at the same route fails the Next build (plan D14). `gateway.chat.stream`
  (`app/_components/gateway-state.ts`) counts only when the service's state is `available`; a
  `degraded` service can still carry the descriptor of its last good probe, and treating that
  stale descriptor as a live capability would report a feature the gateway cannot currently serve.

## End-to-end tiers

- **A Playwright `globalSetup` runs in another process than the tests.** A fake server that a test
  must script, or whose calls a test must count, cannot start there. The iam-console e2e tier only
  checks the staged build in `tests/e2e/global-setup.ts` (SMA-655: `build` stages `.next/static`), and starts the fake IAM,
  the fake IdP, the TLS terminator and the standalone server in a WORKER-scoped fixture
  (`tests/e2e/support/harness.ts`, `workers: 1`). Playwright starts a new worker after a failed test,
  and the fixture then starts the whole stack again. Anything that fixture imports runs WITHOUT the
  vitest `server-only` stub, so `tests/support/` must not import a guarded `@paigasus/sdk` entry or a
  `lib/` file (use `@paigasus/proto/iam`, which the `apps/*/tests/support/**` boundary exemption allows).
- **An e2e tier must never write into a build tree** (MEASURED, SMA-655). iam-console's standalone
  tree serves two tiers — `iam-console-ts:test-e2e` and gateway-console's two-zone tier — and Moon
  runs them at the same time. When each tier's `global-setup.ts` deleted and re-copied
  `.next/static` there, one tier's delete wiped the tree under the other: `ENOTEMPTY` in one
  setup, and `React never hydrated` in 16 of 17 specs of the other. Each app's `build` script now
  stages the standalone tree (`.next/static`, and `public/` if it exists), and the setups only
  check it through `tests/e2e/support/staged-build.ts`. Two tests pin this in each app's `test`
  task: `tests/unit/e2e-read-only.test.ts` is an allowlist scan that reds any `fs` write form
  (`require`, a dynamic `import(...)` and `process.getBuiltinModule(...)` included — the latter
  two also catch a plain backtick specifier, not only `'`/`"`) in `tests/e2e/**` or
  `playwright.config.ts` (its `ALLOWED_EXCEPTIONS` ships empty), AND separately extracts the
  `test-e2e` task's own `script:` block from `moon.yml` by indentation and checks it against an
  ALLOWLIST, not a denylist of mutating command words — a denylist missed `sed -i`, `truncate`,
  `dd` and a `node -e` fs call — so every non-empty trimmed line must be exactly one of
  `set -euo pipefail` or `pnpm exec playwright test`, any other line reds and is named, and the
  `pnpm exec playwright test` line must be present so an emptied script cannot pass — because a
  copy or delete added directly to that script imports no `fs` module at all, so the fs allowlist
  alone cannot see it — and also resolves `playwright.config.ts`'s `globalSetup`/`globalTeardown`
  and reds if either points outside `tests/e2e/`. `tests/standalone-staging.test.ts` reds if
  `build` stops staging.
  Both `test` tasks list `moon.yml` as an input, because without it a `moon.yml`-only edit selects
  neither — a cost of this: EVERY edit to an app's `moon.yml`, comment-only included, now selects
  that app's whole `test` task. After a bare `pnpm exec next build` the staged tree is gone (Next's
  `cleanDistDir`), and `moon run <app>-ts:build` without `--force` sees an unchanged hash and
  skips; use `--force`. Residuals: the scan does not see a write through `child_process` or
  through a helper outside `tests/e2e/`; a Moon cache-hit restore MERGES into `.next`, so a
  deleted stable-named `public/` file can survive; nothing asserts that a third console app has
  these tests.
- **`gateway-console-ts:test-e2e` now needs Docker** (SMA-512 PR 4). It fails loudly when Docker is
  not reachable. `iam-console`'s own e2e tier does not need Docker. The `PAIGASUS_SESSION_STORE`
  memory setting is refused when `PAIGASUS_ZONES` names two zones, so a two-zone tier has no
  alternative store to use instead. Every `iam-console` edit now runs this tier too. An `inputs`
  entry on `gateway-console-ts:test-e2e` causes this, not a `deps` relation — only `inputs` confers
  affectedness on Moon 2.5.3. This is the cost of a tier that must re-run when the property it
  tests can break. A Playwright **worker fixture** now starts a container: the first one in this
  repository started this way. A worker restart does not overlap two containers. The fixture's
  teardown runs before the worker restarts. The old container stops and is fully removed before the
  new worker starts a new one. A measured run showed the old container up at t=15s, no container at
  all at t=16s, and a brand-new container at t=17s. So this pull request needed no deterministic
  container label and no stale-container sweep.
- Playwright's `locator.waitFor()` defaults to **no timeout** (1.63.0), and no `playwright.config.ts`
  in this repo sets `use.actionTimeout`. So an unbounded `waitFor` is bounded only by the TEST
  budget — 120 s in CI, 60 s locally — and when it expires it reports the locator, not a cause.
  That cost SMA-512 pull request 4 two minutes of CI for an unexplained R4 flake. Both consoles'
  hydration waits now go through `tests/e2e/support/hydration.ts`, bounded at
  `HYDRATION_TIMEOUT_MS = 15_000` (one value, deliberately not a `process.env.CI` branch: both
  configs set `retries: isCI ? 2 : 0`, so a branched constant would put the TIGHTER bound on the
  run with NO retry). The helper takes a hand-written STRUCTURAL page type rather than `Page`,
  which lets `tests/unit/hydration.test.ts` drive it with a plain stub and exercise the failure
  path with no browser; a real `Page` satisfies that type on its own, so the module needs no
  `@playwright/test` import at all — not even a type one. Two traps measured there:
  `Pick<Page, 'locator'>` does NOT accept a stub (it keeps the full `Locator` return type —
  `error TS2322`), so the parameter is a structural type; and the helper verifies timeout
  failures by checking the error's `name` field. MEASURED on 1.63.0: a
  `waitFor` that exceeds its own `timeout` rejects with `name` `TimeoutError`, but the constructor
  name is mangled to `TimeoutError2` by bundling, so `error.constructor.name` is not usable either;
  the check is on `name` because importing the class would give the module a runtime
  `@playwright/test` dependency. Playwright also rejects a pending `waitFor` with "Target page, context
  or browser has been closed" during teardown; calling that "the client bundle did not run" is a
  confident wrong diagnosis. **Residual: nothing gates a third console zone** — a new app that copies
  `login.ts` gets an unbounded wait and no `hydration.test.ts`, and nothing reds. `stripComments`
  (SMA-639 local review) is now a single-pass character scanner, not a pair of regexes: it tracks
  plain code, a single-quoted string, a double-quoted string, a template literal, a line comment,
  and a block comment as separate states, with backslash escapes consumed inside a string. A `/*`
  or `//` inside a string literal, or inside a line comment, no longer starts a comment and can no
  longer eat a real `.waitFor(` call — the false negative the old regex pair had is closed, and a
  fixture in `hydration.test.ts` proves it (verified by temporarily restoring the old two-regex
  version, which fails that fixture). A template literal's `${...}` interpolation is now tracked as
  its own code region too (SMA-639 CR round 2): the scanner resumes plain-code scanning at an
  unescaped `${`, counts nested `{`/`}` pairs to find the matching closer, so an object literal or
  a block body inside the interpolation does not end it early, and a nested template literal inside
  an interpolation is handled the same way, on the same stack. A second fixture in
  `hydration.test.ts` proves this the same way (temporarily masking the whole template span again
  fails that fixture). **What remains:** the scanner is not a full tokenizer, so a regex literal is
  not its own state — a `/*` or `//` sequence inside one would still be read as a comment marker.
  That shape is absent from the tree today and is not gated. The scan's regex also cannot see the sibling
  `waitForURL`/`waitForResponse`/`waitForRequest`/`waitForLoadState` APIs, which default to unbounded
  the same way (`use.navigationTimeout` also defaults to 0) — roughly 15 live call sites across both
  apps' e2e trees are not covered, and widening the regex is out of scope. The `15_000` literal pin in
  `hydration.test.ts` case 5 is also the only tight constraint on the value in CI: MEASURED, with the
  literal removed, a `30_000` constant passes the relational assertions under `CI=1`, since CI's 120 s
  budget permits up to 30 s — the `/4` bound is tight only locally. `@paigasus/app-shell`'s
  `loadHydrated` is NOT affected: it uses `expect(...).toHaveCount(1)`, already bounded by the expect
  timeout.
