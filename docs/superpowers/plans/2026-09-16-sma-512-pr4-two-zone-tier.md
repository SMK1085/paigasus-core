# SMA-512 pull request 4 — the two-zone end-to-end tier

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove acceptance criteria 1 and 2 of SMA-512 — a user signed in at the IAM zone reaches the gateway zone with no re-authentication, and a cold login at the gateway zone works with the `iam-console` app receiving no traffic at all.

**Architecture:** One Redis container, two Next standalone servers, and one TLS terminator that path-routes `/iam/*` and `/gateway/*` behind a single origin — the shape SMA-513's ingress will deploy. The two zones share one session because they share one Redis record and one host-only cookie on one origin. A counting forwarder sits between the terminator and the `iam-console` server, so "the gateway zone never touches the IAM zone" becomes a number rather than an assumption.

**Tech Stack:** Playwright, testcontainers (Redis), Next 16.3.4 standalone, Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-09-13-sma-512-gateway-console-design.md` (revision 2). Read § 9, § 10.1, § 10.5, § 11, § 13 and § 14 before Task 1.

**Base branch:** `main` at `43afaf4a`. **Branch:** `feature/sma-512-two-zone-tier`.

---

## Why this pull request is the point of the issue

SMA-512's own notes say it plainly: *"ACs 1 and 2 are the real deliverable. They are the two properties that distinguish this topology from both 'one big app' and 'separate origins', and neither is verifiable from the IAM zone alone."* Pull requests 1 to 3 built the parts. This one is the only evidence the topology works.

Two of its rows are also the **only** controls in the repository for properties that ship today with nothing enforcing them:

- **The once-per-app `createConsoleRuntime` rule.** Outside a React server render `cache()` is a pass-through, so no unit or integration test can observe a second call. An `Introspect` count for one page render is the only thing that can (spec § 5.3), and both zones ship that rule today with no control behind it.
- **Static-chunk isolation under one origin.** Two Next apps at one origin with different base paths have never been run together in this repository. It rehearses SMA-513's acceptance criterion 3.

---

## Global Constraints

Every task's requirements implicitly include this section.

- **SPDX header on every source file:** `// SPDX-License-Identifier: Apache-2.0` as the first line.
- **Conventional commits with a workspace scope.** The subject must not start with an upper-case token, and **no body line may start with `word:`** — such a line parses as a footer token and fails `footer-leading-blank`.
- **Extensionless relative value imports.** Turbopack does not resolve `'./a.js'` to `a.ts`.
- **`PAIGASUS_SESSION_STORE=redis` is mandatory here, not a preference.** `createAuthRuntime` throws when the store is `memory` and `PAIGASUS_ZONES` names more than one zone. A two-zone stack on the memory store cannot start, and that is intended: under it a user signed in on one zone is anonymous on the other.
- **What must match across the two zones** (spec § 11): one `PAIGASUS_SESSION_REDIS_URL` with the key prefix left empty so both zones read the same records; one `PAIGASUS_PUBLIC_ORIGIN`, because the cookie is host-only on that origin; one OIDC client with one redirect URI per zone; and a **byte-identical** `PAIGASUS_ZONES` JSON in both.
- **A Playwright `globalSetup` runs in a different process from the tests.** Anything a test must script, count, or restart belongs in the worker-scoped fixture. Playwright starts a new worker after a failed test, so the fixture must be able to bring the whole stack up again — the container included.
- **Docker is required and must fail loudly.** No skip hatch, following the precedent in `ts/packages/paigasus-discovery/vitest.containers.config.ts`. A tier that skips when Docker is unreachable reports green having proved nothing, which is the failure this whole issue exists to avoid.
- **Never run `git add -A`.** `scratchpad-types-orig.ts` is untracked at the repository root and must stay untracked.
- **macOS has no `timeout`.** And a pipeline's exit status is the LAST command's: `set -e` does not stop a script after `some_command | tail -3` fails. Never gate a destructive step on a piped command's apparent success — that mistake was made twice in this issue's earlier sessions.
- **Moon needs the shims on `PATH`:** `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- **The local bash split:** `ci/affected-graph/run.sh` needs system `/bin/bash` 3.2; `ci/ruff/run.sh` and `ci/next-public/run.sh` need bash 4+; `ci/actionlint/run.sh` has **no working local bash** and its verdict comes from CI only.

---

## Rulings this plan makes

**D19 — the two-zone specs live in `gateway-console-ts:test-e2e`, as spec § 10.5 says, and that makes the WHOLE task need Docker.** The single-zone rows R1 to R7 do not need a container, but they share the task, so a developer with no Docker daemon loses all of them rather than five. The alternative — a separate `test-e2e-two-zone` Moon task — costs a new task name in `ci.yml`'s `T=(…)` array and in the `CLAUDE.md` mirror, both of which `repo:affected-smoke` asserts. The spec chose one task; this plan follows it and records the cost in Known Limits.

**D20 — the two-zone stack is a SEPARATE worker fixture, in its own Playwright project.** The single-zone stack runs one server on the memory store; this one runs two servers, a container and a forwarder. Forcing both through one fixture would make every single-zone row pay for a container. Two projects in one config, selected by `testMatch`, keep the stacks apart and keep `workers: 1`.

**D21 — the spec files stay FLAT in `tests/e2e/`, named `two-zone-*.spec.ts`.** `tests/unit/e2e-rows.test.ts` reads that directory non-recursively, so a subdirectory would take the new rows outside the coverage guard. Flat names keep the guard working and still give the Playwright project a clean `testMatch`.

**D22 — the counting forwarder is app-local, not a shared double.** It goes in `ts/apps/gateway-console/tests/e2e/support/`, not `@paigasus/console-core/testing`. The four shared doubles stand in for real services; this is a measuring instrument for one tier. Move it only when a second tier needs it.

**D23 — `startTlsTerminator` keeps its single-upstream form.** `target` and the new `routes` are mutually exclusive, and passing both is an error rather than a precedence rule. `iam-console`'s tier and the gateway zone's single-zone tier both call the existing form, and neither should change in this pull request.

---
## What the survey established, so no task re-derives it

Measured against `main` at `43afaf4a`.

- **The Redis container shape** the repository already uses, in five places:
  `new GenericContainer('redis:8-alpine').withExposedPorts(6379).start()`, with the URL built as
  `redis://${container.getHost()}:${container.getMappedPort(6379)}/0` and a `120_000` ms start budget.
- **`keyPrefix` is hardcoded `''`** in `ts/packages/paigasus-auth/src/runtime.ts:147-148`, with the comment
  "All zones share ONE store — no per-zone key prefix to configure". That is what makes one Redis record
  readable from both zones, and it needs no configuration in this tier.
- **The exact multi-zone guard**, `runtime.ts:112`:
  `'PAIGASUS_SESSION_STORE cannot be "memory" when PAIGASUS_ZONES declares more than one zone'`.
- **`startTlsTerminator` has THREE call sites**, all passing `{ target, tls }`: `gateway-console`'s harness
  (`:148`), `iam-console`'s harness (`:144`), and `iam-console/tests/integration/doubles/tls-terminator.test.ts:35`.
  Ruling D23 keeps all three working untouched.
- **Only TWO affected-graph cases anchor under `ts/apps/iam-console/`** — `iam-console-lib->iam-console-tasks`
  (`run.sh:595`) and `iam-console-proxy->iam-console-tasks` (`:597`), both currently expecting
  `iam-console-ts:build,iam-console-ts:test,iam-console-ts:test-e2e,ts:lint`.
- **`ts/eslint.config.js`, `SOURCE_ONLY_PACKAGES`, `BOUNDARY_SCOPES` and `APP_CONFIG_FLOOR` need no change.**
  Pull request 3 settled all four; this pull request adds no app and no package.

## What this pull request does NOT change, so no task hunts for it

- **`.github/workflows/ci.yml`** — pull request 3 already installs Playwright Chromium for `gateway-console`,
  and this adds no app. GitHub's runners provide a Docker daemon, which the `@paigasus/auth` and
  `@paigasus/discovery` container tiers already rely on, so the `moon ci` job needs no change.
- **`.github/CODEOWNERS`** — Moon-generated from the project list, and this adds no project.
- **`ci.yml`'s `T=(…)` array and the `CLAUDE.md` mirror** — `:test-e2e` is already a member. That is precisely
  why ruling D19 keeps the two-zone rows inside the existing task.
- **`ci/next-public/run.sh`, `ts/eslint.config.js`, `SOURCE_ONLY_PACKAGES`, `BOUNDARY_SCOPES`** — all settled
  by pull request 3.
- **`ts/pnpm-workspace.yaml` and the pnpm catalog** — `testcontainers` is already used by two packages, so it
  needs a `devDependencies` entry in this app and no catalog entry.

## The one thing nobody here has done before

**No Playwright worker fixture in this repository starts a container.** The only container-in-Playwright case
is `@paigasus/auth`, and it starts Redis at `playwright.config.ts` module-evaluation time, not in a fixture —
because a `webServer` block needs the URL before `globalSetup` would run. Its `global-setup.ts` also carries a
cross-process "adopt if reachable" memo, added after `docker ps` showed **two** container pairs for one
`playwright test` run, since Playwright workers are separate OS processes.

Spec § 10.5 nevertheless requires the fixture, and it gives the reason: a `globalSetup` runs in another
process, and Playwright starts a new worker after a failed test, so only a fixture can bring the whole stack
back. Spec § 14 item 2 therefore asks this plan to **measure** the pattern rather than assume it, including
what a worker restart does to a container that is already running.

Task 3 opens with that measurement, and its answer decides the harness. Do not skip it.

---

### Task 1: The terminator's path router

**Files:**
- Modify: `ts/packages/paigasus-console-core/testing/tls-terminator.ts`
- Test: `ts/apps/gateway-console/tests/integration/doubles/tls-terminator-routes.test.ts`

**Interfaces:**
- Produces: `startTlsTerminator({ tls, routes })`, where `routes` is `readonly { prefix: string; target: string }[]`.
  Task 3's harness consumes it.
- The existing `{ tls, target }` form is unchanged and its three call sites keep working (ruling D23).

- [ ] **Step 1: Write the failing routing test**

`ts/apps/gateway-console/tests/integration/doubles/tls-terminator-routes.test.ts`. Start two trivial
`node:http` servers that echo which one answered, front them with one terminator, and assert:

```ts
it('routes by path prefix, longest prefix first', async () => { /* /iam/x -> A, /gateway/x -> B */ });
it('routes a nested path under a prefix', async () => { /* /gateway/_next/static/x.js -> B */ });
it('keeps Host unchanged and sets X-Forwarded-Proto on a routed request', async () => { /* … */ });
it('answers 502 for a path no route matches', async () => { /* / -> 502 */ });
it('rejects both `target` and `routes` in one call', async () => {
  await expect(startTlsTerminator({ tls, target: 'http://127.0.0.1:1', routes: [] })).rejects.toThrow(/target|routes/);
});
it('rejects an empty routes array', async () => { /* … */ });
```

The nested-path case is the one that matters for the tier: static chunks live at `/gateway/_next/static/…`,
and a router that matched only the exact prefix would send every asset to the fallback.

Run the file and watch every case fail before writing the implementation.

- [ ] **Step 2: Extend `startTlsTerminator`**

Keep `forward`'s body — the hop-by-hop filter, the `Host` pass-through, `x-forwarded-proto`,
`x-forwarded-host`, `x-forwarded-port`, the 502 on an upstream error, and the `res.on('close')` destroy — and
change only **which upstream** it picks:

```ts
export type TerminatorRoute = { readonly prefix: string; readonly target: string };

export async function startTlsTerminator(opts: { tls: TlsMaterial; target?: string; routes?: readonly TerminatorRoute[] }): Promise<{ origin: string; close(): Promise<void> }> {
  // Mutually exclusive, and an error rather than a precedence rule: a caller that passes both has a
  // wrong mental model of which upstream serves a path, and silently preferring one would hide it.
  if ((opts.target === undefined) === (opts.routes === undefined)) {
    throw new Error('tls-terminator: pass exactly one of `target` (one upstream) or `routes` (path-routed upstreams)');
  }
  if (opts.routes !== undefined && opts.routes.length === 0) {
    throw new Error('tls-terminator: `routes` must not be empty');
  }
  // LONGEST PREFIX FIRST, so a future '/iam/admin' route wins over '/iam' regardless of array order.
  const routes: readonly TerminatorRoute[] =
    opts.routes === undefined ? [{ prefix: '/', target: opts.target as string }] : [...opts.routes].sort((a, b) => b.prefix.length - a.prefix.length);
  …
}
```

Match a request path against a prefix so that `/gateway` matches `/gateway`, `/gateway/` and
`/gateway/anything`, but **not** `/gatewayx`:

```ts
function matches(pathname: string, prefix: string): boolean {
  if (prefix === '/') return true;
  return pathname === prefix || pathname.startsWith(`${prefix}/`);
}
```

When no route matches, answer **502 with a message naming the path and the configured prefixes**, not a
silent 404 — an unrouted path in this tier means the harness is wired wrongly, and it must say so.

- [ ] **Step 3: Prove the three existing call sites still work**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run iam-console-ts:test
moon run gateway-console-ts:test
moon run gateway-console-ts:test-e2e
moon run iam-console-ts:test-e2e
```

Expected: all PASS. The `{ target, tls }` form is what all three call, and a regression there breaks two
apps' tiers rather than this one.

- [ ] **Step 4: Commit**

```bash
git add ts/packages/paigasus-console-core/testing/tls-terminator.ts ts/apps/gateway-console/tests/integration/doubles/tls-terminator-routes.test.ts
git commit -m "test(ts): path-route the TLS terminator between two upstreams (SMA-512)"
```

---

### Task 2: The counting forwarder

The instrument acceptance criterion 2 rests on. Spec § 10.5 is explicit about why it is a **forwarder** and
not a spy that binds the port: both standalone servers run in this tier, so the `iam-console` server already
owns its port. A bind-and-drop spy would work only with the IAM app stopped, and stopping it is exactly the
tautological test revision 1 of the spec shipped and revision 2 rejected — the `iam-console` process is on
none of the gateway zone's cold-login path, so that test passed whether or not the design had the property.

**Files:**
- Create: `ts/apps/gateway-console/tests/e2e/support/counting-forwarder.ts`
- Test: `ts/apps/gateway-console/tests/integration/doubles/counting-forwarder.test.ts`

**Interfaces:**
- Produces: `startCountingForwarder({ target })` returning `{ url, connections(), requests(), close() }`.
  Task 3's harness puts it between the terminator and the `iam-console` server.

- [ ] **Step 1: Write the failing test**

Assert, against a trivial upstream:

- a request through the forwarder reaches the upstream and the response body arrives intact;
- `connections()` counts **inbound TCP connections**, and `requests()` counts **HTTP requests** — they differ
  under keep-alive, and acceptance criterion 2 wants the connection count;
- both counters are readable as a **snapshot**, so a test asserts a delta rather than an absolute. The whole
  stack is worker-scoped and Playwright restarts the worker after a failure, so an absolute count is a sum
  over every earlier test in that worker — the same defect the spec corrected for the `/authorize` count;
- request headers and the method reach the upstream unchanged;
- a streamed response body is piped through without buffering the whole of it.

- [ ] **Step 2: Write the forwarder**

A plain `node:http` server. Count in a `connection` listener (TCP) and in the request handler (HTTP), proxy
with `http.request` the way the terminator does, and forward hop-by-hop-filtered headers. Keep it under 80
lines; it is an instrument, not a service.

State in the header comment that it counts connections to the **`iam-console` app**, and that the gateway
zone legitimately talks to the IAM **service** — the two are different things, and acceptance criterion 2
concerns only the first (spec § 13).

- [ ] **Step 3: Run and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts/apps/gateway-console exec vitest run tests/integration/doubles/counting-forwarder.test.ts
git add ts/apps/gateway-console/tests/e2e/support/counting-forwarder.ts ts/apps/gateway-console/tests/integration/doubles/counting-forwarder.test.ts
git commit -m "test(ts): a counting forwarder for the gateway zone's isolation proof (SMA-512)"
```

---
### Task 3: The two-zone harness

**Files:**
- Create: `ts/apps/gateway-console/tests/e2e/support/two-zone-harness.ts`
- Modify: `ts/apps/gateway-console/tests/e2e/support/paths.ts` (add the `iam-console` standalone directory)
- Modify: `ts/apps/gateway-console/tests/e2e/global-setup.ts` (check and stage **both** builds)
- Modify: `ts/apps/gateway-console/playwright.config.ts` (two projects)
- Modify: `ts/apps/gateway-console/package.json` (`testcontainers` as a devDependency)

**Interfaces:**
- Produces: `test`, `expect` and `TwoZoneHarness` from `two-zone-harness.ts`, with `origin`, `iam`, `idp`,
  `gateway`, `iamConsole` (the counting forwarder), `url(path)`, `serverOutput(zone)` and `useWorld()`.
  Task 4's five specs import `test` from this module and nothing else.

- [ ] **Step 1: MEASURE the worker-fixture container pattern before building on it**

Spec § 14 item 2. Nothing in this repository starts a container from a Playwright worker fixture, and the one
place that starts containers for a Playwright run does it at config module-evaluation time with a
cross-process memo, added after `docker ps` showed two container pairs for one run.

Write a throwaway spec — `tests/e2e/two-zone-probe.spec.ts`, deleted at the end of this step — with a
worker-scoped fixture that starts `new GenericContainer('redis:8-alpine').withExposedPorts(6379).start()`,
and answer all three questions with evidence, not reasoning:

1. Does the container start and accept a connection at
   `redis://${container.getHost()}:${container.getMappedPort(6379)}/0`?
2. With `workers: 1`, does ONE run start exactly one container? Check with `docker ps` **while the run is in
   flight**, not after.
3. Force a failure in the first of two tests so Playwright restarts the worker. Does the fixture's teardown
   run before the restart? Is the first container stopped, or does a second appear alongside it?

Record all three answers in the task report verbatim, with the `docker ps` output. **If question 3 shows a
leak**, the fixture needs a deterministic label plus a sweep of stale containers at start, the shape
`@paigasus/auth`'s reachability memo takes for the same reason — decide it here, implement it in Step 3, and
say in the report that you did.

Delete the probe spec before moving on. Its output is the measurement, not code to keep.

- [ ] **Step 2: Stage both builds**

`paths.ts` gains the second app's standalone directory. The `iam-console` app's own path is derived from the
`ts/` root, not by composing `'..'` onto this app's directory:

```ts
export const IAM_CONSOLE_APP_DIR = path.join(TS_ROOT, 'apps', 'iam-console');
export const IAM_CONSOLE_STANDALONE_DIR = path.join(IAM_CONSOLE_APP_DIR, '.next', 'standalone', 'apps', 'iam-console');
```

`global-setup.ts` must now assert **both** `server.js` files exist and copy **both** `.next/static` trees into
their standalone counterparts. The standalone output carries no `static` tree, and without the copy every
client chunk is a 404, nothing hydrates, and the cross-zone navigation row cannot run. Name the missing app in
the error message — "run `moon run <app>-ts:build` first" — because with two builds a bare message does not
say which one is absent.

- [ ] **Step 3: Write the harness**

The stack, and the order it must start in:

```
browser --https--> TLS terminator ---- /iam/*     --> counting forwarder --http--> iam-console server.js
                                    \- /gateway/* ------------------------------> gateway-console server.js
                                                          both --h2c--> fake IAM (gRPC)
                                                          both --http--> fake IAM + fake gateway (service-info)
                                                          both --https--> fake IdP
                                                          both --------> Redis container (one session store)
```

Order matters and is forced by a cycle: both servers need `PAIGASUS_PUBLIC_ORIGIN`, which is the terminator's
origin, and the terminator needs the servers' ports. Break it exactly as the single-zone harness does —
allocate the ports first, then build the front, then spawn the servers onto those ports:

1. `testTls()`, then the Redis container, the fake IdP, the fake IAM and the fake gateway.
2. `freePort()` twice, for the two servers.
3. `startCountingForwarder({ target: 'http://127.0.0.1:<iamPort>' })`.
4. `startTlsTerminator({ tls, routes: [{ prefix: '/iam', target: forwarder.url }, { prefix: '/gateway', target: 'http://127.0.0.1:<gatewayPort>' }] })`.
5. Spawn both `server.js` children on their ports.
6. `waitForHealth` on **both** `/<zone>/healthz`, through `127.0.0.1:<port>` directly rather than through the
   terminator, so a health failure names the server rather than the front.

The environment each server gets, with the cross-zone invariants from spec § 11 made explicit:

```ts
const ZONES = JSON.stringify({ iam: '/iam', gateway: '/gateway' });   // BYTE-IDENTICAL in both zones
const shared = {
  NEXT_TELEMETRY_DISABLED: '1',
  NODE_EXTRA_CA_CERTS: tls.certPath,
  PAIGASUS_ZONES: ZONES,
  PAIGASUS_OIDC_ISSUER: idp.issuer,
  PAIGASUS_OIDC_CLIENT_ID: idp.clientId,          // ONE OIDC client, one redirect URI per zone
  PAIGASUS_OIDC_CLIENT_SECRET: idp.clientSecret,
  PAIGASUS_PUBLIC_ORIGIN: terminator.origin,      // ONE origin: the cookie is host-only on it
  PAIGASUS_SESSION_STORE: 'redis',                // MANDATORY: memory + two zones throws at startup
  PAIGASUS_SESSION_REDIS_URL: redisUrl,           // ONE url; keyPrefix is hardcoded '' in @paigasus/auth
  PAIGASUS_SERVICES: JSON.stringify({ iam: iam.httpUrl, gateway: fakeGateway.url }),
  PAIGASUS_IAM_GRPC_URL: iam.grpcUrl,
  PAIGASUS_DISCOVERY_NEGATIVE_MS: '1',
  PAIGASUS_DISCOVERY_FRESH_MS: '2',
  PAIGASUS_DISCOVERY_STALE_MS: '3',
};
// Only PAIGASUS_ZONE and PORT differ between the two.
```

Reuse the single-zone harness's `serverEnv`, `stop`, `waitForHealth`, `closeInOrder` and `freePort` by
**importing them**, not by copying: export them from `harness.ts` if they are not already exported. Two
divergent copies of a process-lifecycle helper is exactly the duplication a reviewer should reject, and the
single-zone harness is already the tested one.

Close in reverse order, through `closeInOrder`, with the container **last**: both servers, the terminator, the
forwarder, the three fakes, then Redis. A start step that throws must close everything that started before it.

`useWorld()` resets the fake IAM's handlers and both descriptors, and restores `gateway.setReachable(true)` —
the same three resets the single-zone harness does, for the same reason.

The worker fixture's timeout must cover the container start on top of the two servers. Budget it explicitly
and say so in a comment: `120_000` for Redis, plus `MAX_START_ATTEMPTS * READY_TIMEOUT_MS` for the servers,
plus room for the fakes and the cleanup.

**Docker fails loudly.** Do not add a skip hatch, a canary or an environment escape. If the daemon is
unreachable the container start rejects and the fixture fails, which is the whole point: a tier that skips
reports green having proved nothing. Say this in the file's header comment, citing
`ts/packages/paigasus-discovery/vitest.containers.config.ts`'s precedent.

- [ ] **Step 4: Two Playwright projects**

Keep `workers: 1` and `fullyParallel: false`. Select the stacks by file name (ruling D21 keeps the specs flat,
so `tests/unit/e2e-rows.test.ts`'s non-recursive scan still sees them):

```ts
projects: [
  { name: 'single-zone', testMatch: /^(?!.*two-zone).*\.spec\.ts$/, use: { ...devices['Desktop Chrome'] } },
  { name: 'two-zone', testMatch: /two-zone.*\.spec\.ts$/, use: { ...devices['Desktop Chrome'] } },
],
```

Write in a comment why there are two: the single-zone rows run one server on the memory store, and forcing
them through the two-zone fixture would make every one of them pay for a container.

**One caveat to state in the comment, because it is not obvious.** Playwright tears worker fixtures down when
the worker ends, not between files, so with one worker both stacks can be alive at once. That is safe here —
every port is ephemeral and every fake binds `127.0.0.1:0` — but it does mean the two-zone project's
container may outlive the last two-zone spec by the length of the single-zone project. Do not "fix" that by
sharing one fixture; the cost of a container on every single-zone row is worse.

- [ ] **Step 5: Add the dependency and prove the stack comes up**

`testcontainers` joins `devDependencies` as `catalog:` — spec § 9 records that it needs no new catalog entry,
because `@paigasus/auth` and `@paigasus/discovery` already use it. Then `pnpm -C ts install`.

Write one throwaway smoke spec that signs in at `/iam/orgs` and asserts the page renders, run it, and confirm
from the server output that **neither** server logged the multi-zone store error. Then delete it — Task 4
writes the real rows.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run iam-console-ts:build gateway-console-ts:build
pnpm --dir ts/apps/gateway-console exec playwright test --project=two-zone
```

- [ ] **Step 6: Commit**

```bash
git add ts/apps/gateway-console ts/pnpm-lock.yaml
git commit -m "test(ts): a two-zone harness with a shared Redis session store (SMA-512)"
```

---
### Task 4: The five rows

Spec § 10.5's table. Each is one Playwright test whose title starts with its row id, because
`tests/unit/e2e-rows.test.ts` pins that mapping and runs in `gateway-console-ts:test` on every pull request
that touches the app — even when the e2e task does not run.

**Files:**
- Create: `ts/apps/gateway-console/tests/e2e/two-zone-session.spec.ts` (R8, R9)
- Create: `ts/apps/gateway-console/tests/e2e/two-zone-isolation.spec.ts` (R10)
- Create: `ts/apps/gateway-console/tests/e2e/two-zone-runtime.spec.ts` (R11)
- Create: `ts/apps/gateway-console/tests/e2e/two-zone-assets.spec.ts` (R12)
- Modify: `ts/apps/gateway-console/tests/unit/e2e-rows.test.ts` (add R8 to R12 to `ROWS`)

- [ ] **Step 1: R8 — a user signed in at the IAM zone reaches the gateway zone with no re-authentication (AC 1)**

```
test('R8: a session from the IAM zone carries into the gateway zone with no second authorization (AC 1)', …)
```

Sign in at `/iam/orgs`. **Snapshot the fake IdP's `/authorize` count**, navigate to `/gateway/overview`, then
assert the overview rendered **and the delta is exactly zero**.

The delta is the whole point. Spec § 10.5 records why an absolute count cannot work: the fixture is
worker-scoped, Playwright restarts the worker after a failed test, and the counter is therefore the sum over
every login that ran before it in that worker. "Exactly one" is flaky; "at least one" asserts nothing.

If `FakeIdp` exposes no `/authorize` counter, add one to
`ts/packages/paigasus-console-core/testing/fake-idp.ts` — readable as a plain number, so a caller snapshots it
— and say in your report that you had to. Do not substitute the token-issuance count without saying so: a
token is issued at the callback, and the property under test is that the browser never reaches `/authorize`
at all.

- [ ] **Step 2: R9 — cross-zone is a hard navigation, same-zone is not (ADR-0017)**

```
test('R9: the cross-zone link is a hard navigation and the same-zone one is not (ADR-0017)', …)
```

Use the technique `ts/packages/paigasus-app-shell/tests/e2e/zones.spec.ts:45-82` established, which is the
only one that distinguishes the two reliably: record every **document** request, set a marker in the page, and
after the click assert both halves.

- Cross-zone (the IAM zone's nav entry for the gateway, or the gateway zone's for IAM): exactly one document
  request for the target path, **and the marker is gone** — the document was replaced.
- Same-zone (the Overview entry from the scope route, both in the gateway zone): **no** document request, and
  **the marker survives**.

Assert both in one test. A cross-zone assertion alone passes for an app where *every* navigation is hard,
which would be a real regression in the other direction.

- [ ] **Step 3: R10 — a cold login at the gateway zone never touches the IAM zone's app (AC 2)**

```
test('R10: a cold login at the gateway zone reaches the overview with zero connections to the iam-console app (AC 2)', …)
```

Snapshot `harness.iamConsole.connections()`, perform a **cold** login at `/gateway/overview` — no prior visit
to `/iam` in this test, and Playwright gives each test a fresh context — then assert the overview rendered and
**the delta is zero**.

State in a comment what this does and does not claim, because the distinction is the reason revision 1 of the
spec had to be rewritten: the gateway zone's cold-login path is proxy → `/gateway/auth/login` → the IdP →
`/gateway/auth/callback` → IAM **gRPC**. The IAM **service** is on that path and must stay up. The
`iam-console` **app** is on none of it, which is what this row proves. Stopping the app instead would pass
whether or not the design had the property, and would keep passing against a design that grew a cross-zone
dependency.

Add a **vacuity guard**: assert the forwarder's counter is greater than zero somewhere in the run — R8 drives
traffic through it — so a forwarder that was never wired into the terminator cannot make this row pass by
counting nothing. Put that assertion in R8, where traffic through `/iam` is expected, not here.

- [ ] **Step 4: R11 — one request renders with exactly one `Introspect` (spec § 5.3)**

```
test('R11: no single request makes more than one Introspect call (§ 5.3)', …)
```

This is the **only** control in the repository for the once-per-app `createConsoleRuntime` rule. Outside a
React server render `cache()` is a pass-through, so no unit or integration test can observe a second call.

**Do not assert a raw count of `Introspect` calls for one navigation.** After hydration the router prefetches
visible links, and each prefetch is another request and therefore another legitimate render and another
legitimate `Introspect`. A raw count is flaky in the direction that matters — it fails when the product is
correct.

Assert the per-request invariant instead. `proxy.ts` mints a fresh correlation id per request and
`@paigasus/console-core` sends it to IAM, so the fake's call log carries it:

```ts
const calls = harness.iam.callsTo('authn.introspect');
const byCorrelation = new Map<string, number>();
for (const call of calls) {
  if (call.correlationId === null) continue;
  byCorrelation.set(call.correlationId, (byCorrelation.get(call.correlationId) ?? 0) + 1);
}
// Vacuity: the grouping must have seen something, and the ids must be real.
expect(byCorrelation.size).toBeGreaterThan(0);
expect(calls.filter((call) => call.correlationId === null)).toEqual([]);
// The invariant: ONE runtime per request means ONE Introspect per request, however many requests ran.
expect([...byCorrelation.entries()].filter(([, count]) => count > 1)).toEqual([]);
```

A second `createConsoleRuntime()` call makes a second memoization identity **within the same request**, so its
two `Introspect` calls carry the **same** correlation id and land in one group — which is exactly what the
last assertion catches, and what a raw count would only catch by luck.

Explain all of that in the test's header comment. A future reader who "simplifies" this to a count will
reintroduce the flake and weaken the only control this rule has.

- [ ] **Step 5: R12 — the two zones' static assets do not collide under one origin**

```
test('R12: each zone loads its own _next assets under its own base path (SMA-513 AC 3 rehearsal)', …)
```

Record every response whose pathname contains `/_next/static/`. Visit `/iam/orgs`, then `/gateway/overview`,
and assert:

- every static request made during the IAM visit is under `/iam/_next/`, and every one during the gateway
  visit is under `/gateway/_next/`;
- every one answered **200** — a 404 here is the collision, since a chunk requested at the wrong prefix is
  what a shared `_next` root would produce;
- at least one stylesheet and one script in each zone, so the row cannot pass by observing nothing;
- the two zones' visits both rendered — assert a visible element from each.

This rehearses SMA-513's acceptance criterion 3. Two Next apps have never run at one origin in this
repository before, and the base path is the only thing separating their asset roots.

- [ ] **Step 6: Extend the row-coverage guard**

`tests/unit/e2e-rows.test.ts`: add `'R8', 'R9', 'R10', 'R11', 'R12'` to `ROWS`, and update the header comment
— spec § 10.5 contributes five rows, on top of § 10.4's six and this zone's two additions. The guard's
`readdirSync` is **not** recursive, which is why ruling D21 keeps the new spec files flat in `tests/e2e/`.

- [ ] **Step 7: Run the tier and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run gateway-console-ts:test          # the row guard runs here
moon run gateway-console-ts:test-e2e      # both projects: 8 single-zone rows + 5 two-zone rows
```

Report the two-zone project's wall-clock time separately from the single-zone project's — spec § 14 item 5
wants it before anyone reasons about CI cost.

```bash
git add ts/apps/gateway-console/tests
git commit -m "test(ts): prove the cross-zone session and the gateway zone's isolation (SMA-512)"
```

---
### Task 5: The affectedness edge, and the measurement that proves it

The tier is worthless if it does not re-run when the property it tests can break. Spec § 9 records that
revision 1 of the design got this wrong: it claimed the `iam-console -> gateway-console` edge came from the
`deps` relation. It does not. **Task `inputs` are the only thing that confers affectedness in Moon 2.5.3**;
`dependsOn` and `^:build` schedule an upstream's build and never select a downstream. The affected-graph
harness runs a plain `moon query tasks --affected` with no graph flags, so under revision 1 the case would
have reported an empty set and the only proof of acceptance criteria 1 and 2 would never have re-run on an
`iam-console` change.

**Files:**
- Modify: `ts/apps/gateway-console/moon.yml` (`test-e2e` inputs and deps)
- Modify: `ci/affected-graph/run.sh` (two re-baselines, one new case)

- [ ] **Step 1: Add the input edge and the build dependency**

`gateway-console-ts:test-e2e`'s `inputs` gain the `iam-console` tree, with the negations that keep build output
out of the hash:

```yaml
      # SMA-512 PR 4. The two-zone tier runs the iam-console standalone server, so an iam-console
      # change can break it. This edge comes from `inputs` and NOT from `deps`: task inputs are the
      # only thing that confers affectedness in Moon 2.5.3, and the affected-graph harness queries
      # with no graph flags. Without this line the tier never re-runs on an iam-console change and
      # the only proof of ACs 1 and 2 goes quietly stale.
      #
      # THE COST IS REAL AND ACCEPTED: every iam-console edit now runs a Docker-backed, two-server
      # Playwright tier.
      - '/ts/apps/iam-console/**/*'
      # `.next` is a declared build output and .moon/workspace.yml's hasher.ignorePatterns
      # deliberately does NOT ignore it, so a blanket tree glob would hash the whole build.
      - '!/ts/apps/iam-console/.next/**'
      - '!/ts/apps/iam-console/tests/fixtures/**/.next/**'
      - '!/ts/apps/iam-console/tests/fixtures/*/next-env.d.ts'
```

and its `deps` gain `'iam-console-ts:build'` — which schedules that build, and is needed **in addition to**
the input, never instead of it.

Check whether `ts/apps/iam-console/tsconfig.tsbuildinfo` needs a fourth negation. It exists on disk and is
gitignored, so it does **not** appear in a `--base origin/main` diff and cannot affect CI selection; add a
negation only if a local run shows it re-keying the task, and do not add one speculatively —
`repo:input-liveness` does not scan this task, but a negation that names nothing is still noise.

- [ ] **Step 2: MEASURE that the edge actually selects the tier (spec § 14 item 4)**

This is the fix for revision 1's wrong claim, and the spec says it must be confirmed rather than assumed.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
printf '%s\n' 'ts/apps/iam-console/lib/config.ts' \
  | moon query tasks --affected \
  | python3 -c '
import sys, json
d = json.load(sys.stdin)
for pid, tasks in (d.get("tasks") or {}).items():
    for name in tasks:
        print(f"{pid}:{name}")'
```

Expected: `gateway-console-ts:test-e2e` is in the output.

**Two traps, both recorded as measured in `CLAUDE.md`, and both easy to fall into here.**

1. **Parse the JSON; do not grep it.** `moon query tasks --affected` emits each selected task's `deps[]`, and
   every dep entry carries a `"target"` key of its own, so `grep -o '"target": "[^"]*"'` counts **scheduled
   upstreams as selections** — it reported 15 tasks where the real answer was 12. Take one target per
   `tasks[project][task]`, as above.
2. **Read the exit status UNPIPED.** A reader in a pipeline returns its own status: `jq` exits 0 on empty
   input, so a failed `moon query` reads as "the reader found nothing" rather than "the query failed". Run
   `moon query tasks --affected` alone first and check `$?` before trusting any pipeline over it.

Record the command and its full output in your report. If `gateway-console-ts:test-e2e` is absent, the edge
does not work and Step 1 is wrong — stop and fix it rather than proceeding to the re-baselines.

- [ ] **Step 3: Re-baseline the two cases anchored under `ts/apps/iam-console/`**

Both currently expect `iam-console-ts:build,iam-console-ts:test,iam-console-ts:test-e2e,ts:lint`. Each gains
`gateway-console-ts:test-e2e` — and **only** that task, because the new input sits on that task alone:

- `iam-console-lib->iam-console-tasks`, `ci/affected-graph/run.sh:595`, anchored on `ts/apps/iam-console/lib/config.ts`
- `iam-console-proxy->iam-console-tasks`, `:597`, anchored on `ts/apps/iam-console/proxy.ts`

Take the exact expected set from a real run, never from this table. Run the suite under **system `/bin/bash`
3.2** — Homebrew's 5.3.15 deadlocks on its here-string reader:

```bash
/bin/bash ci/affected-graph/run.sh 2>&1 | tee /tmp/affected-pr4.txt
```

- [ ] **Step 4: Add the case that states the intent**

The two re-baselined cases pin the edge only as a side effect, and both anchor on files outside `app/` — the
app's largest subtree. Add a third, named for what it protects:

```bash
  # SMA-512 PR 4 — the two-zone tier runs the iam-console standalone server, so ANY iam-console
  # change must re-run it. The two cases above anchor lib/ and proxy.ts; this one anchors app/,
  # the subtree neither reaches. The edge is an `inputs` entry on gateway-console-ts:test-e2e, not
  # a `deps` relation: deps schedule a build and never select a downstream (CLAUDE.md, Moon 2.5.3).
  run_task_case_ci "iam-console-app->two-zone-tier" "ts/apps/iam-console/app/(console)/layout.tsx" \
    "<measured>"
```

- [ ] **Step 5: Run the full gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash ci/affected-graph/run.sh
moon run repo:affected-smoke --force
```

Both must pass. If `repo:affected-smoke` aborts in under three seconds, grep the output for `proto-shim`: a
`Permission denied (os error 13)` under a concurrent `moon ci` is an infrastructure abort at rc 2, not a
verdict. Capture the output **before** re-running — a passing re-run overwrites every artifact holding the
evidence.

- [ ] **Step 6: Commit**

```bash
git add ts/apps/gateway-console/moon.yml ci/affected-graph/run.sh
git commit -m "ci(ts): make an iam-console change re-run the two-zone tier (SMA-512)"
```

The body must carry the measurement from Step 2 — the command and the line proving
`gateway-console-ts:test-e2e` is selected. A re-baseline commit that does not show the query is unreviewable.

---

### Task 6: Documentation and the recorded measurements

**Files:**
- Modify: `CLAUDE.md`
- Modify: `ts/apps/gateway-console/README.md`
- Modify: `docs/superpowers/specs/2026-09-13-sma-512-gateway-console-design.md` (§ 14)

- [ ] **Step 1: `CLAUDE.md`**

Correct the `gateway-console` bullet pull request 3 added, and add what is new:

- `gateway-console-ts:test-e2e` **now requires Docker** and fails loudly without it. `iam-console`'s tier does
  not. The memory store is refused with two zones, so there is no alternative.
- **Every `iam-console` edit now runs that tier**, through an `inputs` entry, not a `deps` relation. That is
  the price of making the tier re-run when the property it tests can break.
- A Playwright **worker fixture** now starts a container — the first in this repository. Record what Task 3
  Step 1 measured about a worker restart, because the next person to try this will hit it.

Two gates constrain this file, and both are easy to trip: a `<!-- moon-diagnosis:ok -->` marker is required on
any file naming `ciReport.json`, and the `<!-- ci-targets:begin -->` / `<!-- ci-targets:end -->` markers must
stay unique — a second copy of either, **even inside backticks in prose**, makes the count 2 and reds
`repo:affected-smoke`.

- [ ] **Step 2: The app README**

The Test tiers section gains the two-zone tier: what it starts, that it needs Docker, that the single-zone
rows share the task and therefore need Docker too (ruling D19), and which rows belong to which project.

- [ ] **Step 3: Close out spec § 14**

Record the answers this pull request produced, in the spec itself:

| Item | Owner | What to record |
|---|---|---|
| 1 — one terminator path-routing two standalone servers, `Host` and `X-Forwarded-Proto` intact, and whether the apps' static chunks collide | Tasks 1 and 4 (R12) | the measured answer, and that it rehearses SMA-513 acceptance criterion 3 |
| 2 — testcontainers from a Playwright **worker fixture**, and what a worker restart does to a running container | Task 3 Step 1 | all three answers with the `docker ps` evidence |
| 4 — whether the `inputs` edge really selects the tier, read with the unpiped exit status | Task 5 Step 2 | the query and its output |
| 5 — the wall-clock cost of the tier on a loaded CI runner | Task 4 Step 7, then CI | local first; the CI number once the pull request has run |

Items 3 and 6 were answered by pull requests 2 and 3 and need no change here.

- [ ] **Step 4: The full-graph run, stated honestly**

No single local bash runs every gate. Report the results separately and say which gate has no local verdict:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :typecheck :test-e2e --base origin/main --include-relations
/bin/bash ci/affected-graph/run.sh                      # needs bash 3.2
/opt/homebrew/bin/bash ci/ruff/run.sh                   # needs bash 4+
/opt/homebrew/bin/bash ci/next-public/run.sh            # needs bash 4+
```

`repo:actionlint` has no working local bash on this class of machine. Its verdict comes from CI.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md ts/apps/gateway-console/README.md docs/superpowers/specs/2026-09-13-sma-512-gateway-console-design.md
git commit -m "docs(ts): record the two-zone tier, its Docker requirement and its measurements (SMA-512)"
```

---

## Known limits this pull request ships

- **The whole `gateway-console-ts:test-e2e` task now needs Docker**, single-zone rows included, because they
  share one task (ruling D19). A developer with no daemon loses thirteen rows, not five.
- **Every `iam-console` edit runs a Docker-backed, two-server Playwright tier.** Accepted deliberately: the
  alternative is a tier that goes stale exactly when the property it tests breaks.
- **A stopped zone app stays invisible to ADR-0020 discovery.** Discovery probes services, not zone apps, so a
  zone whose app is down still renders an *available* nav entry pointing at a dead route. Nothing here detects
  that, and SMA-513's ingress is where it would be caught.
- **Acceptance criterion 3 remains undelivered** (decision D11): the gateway's chat route authenticates
  Paigasus API keys and never an OIDC token, so a console playground could not make one real call. SMA-635
  owns it.
- **The container may outlive the last two-zone spec** by the length of the single-zone project, because
  Playwright tears worker fixtures down at worker end rather than between files (ruling D20).
- **`repo:next-env-drift` still has no negative control** (SMA-637).
- Roughly 250 lines remain byte-identical across the two zones, recorded in spec § 13, with an extraction owed
  before a third zone.
