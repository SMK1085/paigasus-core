# SMA-648 Descriptor Cache Redis Idle Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The descriptor cache's node-redis client in `@paigasus/console-core` survives an idle
socket and recovers from a real hang, logs one `discovery.redis_connection_lost` line per real
loss, and `@paigasus/discovery` refuses a client that would close forever after an idle gap.

**Architecture:** `redisDescriptorCache` adds `pingInterval: timeoutMs` and an exported
`descriptorCacheReconnectStrategy` that returns a delay for every cause. An exported
`watchConnectionLoss(client, log)` replaces the silent error listener. `createRedisDescriptorCache`
asserts two more preconditions (a ping at most half of `socketTimeout`, and a strategy that
accepts a `SocketTimeoutError`). A new Docker-backed `test-e2e` tier in `@paigasus/console-core`
drives the real call site against a real Redis.

**Tech Stack:** TypeScript (Node 24), node-redis (`redis` 6.2.1 / `@redis/client` 6.2.1), vitest 5,
testcontainers 12.1.0, Moon 2.5.3, pnpm, bash (`ci/affected-graph/run.sh`).

**Spec:** docs/superpowers/specs/2026-09-18-sma-648-descriptor-cache-redis-idle-design.md

## Global Constraints

- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-648`, branch `feature/sma-648-ts-descriptor-cache-redis-idle`. Check `git branch --show-current` before the first commit.
- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for shell/YAML). Markdown files carry no SPDX line.
- Conventional commits with a workspace scope: `fix(ts): …`, `test(ts): …`, `test(ci): …`, `docs(ts): …`. Header ≤ 100 characters. Body lines ≤ 100 characters.
- Every commit message ends with the line `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>` after one blank line.
- Do NOT put a line of the form `Word: value` or `#123` in a commit BODY. commitlint reads it as a footer and fails `footer-leading-blank`.
- Never use `--no-verify`. Never use `git commit --amend`. Always add a new commit. Stage files by path; never `git add -A`.
- Never log, print or commit a DSN or a password. Test code reads log lines only to assert that the secret is ABSENT.
- Prefix every moon/pnpm/uv command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Run `pnpm -C ts exec prettier --write <each changed ts/ file>` before each commit that touches `ts/`. `ts:fmt` is a separate whole-tree Prettier gate (printWidth 200).
- The `test-e2e` tiers need Docker. testcontainers finds it through `/var/run/docker.sock` (a symlink to Docker Desktop's socket on this machine). If testcontainers reports that it cannot find a container runtime, `export DOCKER_HOST=unix://$HOME/.docker/run/docker.sock` and retry. Do NOT add a skip hatch.
- The console-core tier timings are fixed by the spec: `PAIGASUS_SESSION_REDIS_TIMEOUT_MS = 500`, so `socketTimeout = 1000`, `pingInterval = 500`. Every poll is bounded at 5000 ms.
- Commits are SSH-signed through 1Password. If `git commit` fails with `failed to fill whole buffer`, 1Password is locked. Stop and report it; do not disable signing.
- If the sandbox refuses a shell loop or a variable in a Bash call, write the commands to a script file in your scratchpad and run it with `/bin/bash <that file>`.

---

## File Structure

| File | Change | Responsibility |
|------|--------|----------------|
| `ts/packages/paigasus-console-core/src/discovery.ts` | modify | D10 seam fix; D2/D3 client options; exported `descriptorCacheReconnectStrategy`, `watchConnectionLoss`, `ConnectionLossSource`, `ConnectionLossReason`; D11 `connectOnce`; header comment. |
| `ts/packages/paigasus-console-core/src/logger.ts` | modify (line 14) | `AppEventName` gains `'discovery.redis_connection_lost'`. |
| `ts/packages/paigasus-console-core/tests/support/discovery-state.ts` | create | Test helper: reads `redisClient` from the `globalThis` state key. |
| `ts/packages/paigasus-console-core/tests/unit/discovery-singleton.test.ts` | modify | D10 unit test. |
| `ts/packages/paigasus-console-core/tests/unit/discovery-redis-client.test.ts` | create | § 5.2 default tier: strategy range, `watchConnectionLoss` cases and reasons, client-options pin. |
| `ts/packages/paigasus-console-core/tests/containers/descriptor-cache-idle.test.ts` | create | § 5.1 container tier, T1–T5. |
| `ts/packages/paigasus-console-core/vitest.containers.config.ts` | create | Container-tier vitest config with the `server-only` alias. |
| `ts/packages/paigasus-console-core/vitest.config.ts` | modify (lines 34-38) | Exclude `tests/containers/**`. |
| `ts/packages/paigasus-console-core/tsconfig.json` | modify (line 9) | `include` gains `vitest.containers.config.ts`. |
| `ts/packages/paigasus-console-core/package.json` | modify (devDependencies) | `testcontainers: "catalog:"`. |
| `ts/packages/paigasus-console-core/moon.yml` | modify (append after line 64) | New `test-e2e` task. |
| `ts/packages/paigasus-console-core/README.md` | create | The package has NO README today. States the Docker requirement of `test-e2e`. |
| `ts/pnpm-lock.yaml` | regenerate | The new devDependency. |
| `ts/packages/paigasus-discovery/src/adapters/redis-cache.ts` | modify (lines 1-2, 49-66, after 92) | D4 (a) and (b), D5 doc corrections. |
| `ts/packages/paigasus-discovery/tests/redis-cache-options.test.ts` | rewrite | Valid default client, unique message fragment per precondition, new D4 rows. |
| `ts/packages/paigasus-discovery/tests/containers/support/redis.ts` | modify (lines 15-25) | `connect()` gains `pingInterval: 5_000` and a reconnect strategy. |
| `ts/packages/paigasus-discovery/README.md` | modify (lines 87-100) | D5 wording, "all four" becomes "all six". |
| `ci/affected-graph/run.sh` | modify (lines 89-99, 494-528, 657-676) | "FIVE" becomes six; measured re-baseline with SMA-648 comments. |
| `CLAUDE.md` | modify (insert before line 1055) | One Gotchas entry in the D5 wording. |

---

## Task 1: D10 — the reset seam survives a closed client

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/discovery.ts:130-136`
- Create: `ts/packages/paigasus-console-core/tests/support/discovery-state.ts`
- Modify: `ts/packages/paigasus-console-core/tests/unit/discovery-singleton.test.ts` (whole file shown)

**Interfaces:**
- Consumes: `descriptorCacheFor(config: ConsoleCoreConfig, log?: ConsoleLogger): DescriptorCache`, `createJsonLogger(write?: (line: string) => void): ConsoleLogger`.
- Produces: `resetDiscoveryForTest(): void` (same signature, no throw on a closed client); test helper `redisClientFromState(): RedisClientType | undefined`.

- [ ] **Step 1: Create the test helper**

Write `ts/packages/paigasus-console-core/tests/support/discovery-state.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Reads the Redis client that src/discovery.ts holds on globalThis (SMA-648 § 5.2). The key string
// is the one src/discovery.ts declares. Symbol.for resolves it through the process-wide registry,
// so this helper needs no export from src/.
import type { RedisClientType } from 'redis';

const DISCOVERY_STATE = Symbol.for('paigasus.console-core.discovery-state.v1');

export function redisClientFromState(): RedisClientType | undefined {
  const holder = globalThis as typeof globalThis & { [DISCOVERY_STATE]?: { redisClient?: RedisClientType } };
  return holder[DISCOVERY_STATE]?.redisClient;
}
```

- [ ] **Step 2: Write the failing test**

Replace `ts/packages/paigasus-console-core/tests/unit/discovery-singleton.test.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// `next dev` recompiles a module graph on every edit, so a MODULE-level singleton is remade each
// time and its Redis client leaks. The fix is the one SMA-511 applied to getAuthRuntime: hold the
// state on globalThis under a Symbol.for key. This test reproduces a recompile with a dynamic
// import that bypasses the module cache.
import { afterEach, describe, expect, it } from 'vitest';
import { descriptorCacheFor, resetDiscoveryForTest } from '../../src/discovery';
import type { ConsoleCoreConfig } from '../../src/config-shape';
import { createJsonLogger } from '../../src/logger';
import { redisClientFromState } from '../support/discovery-state';

const memoryConfig = { PAIGASUS_SESSION_STORE: 'memory' } as ConsoleCoreConfig;

// Port 1 refuses the connection, so no test in this file needs a Redis.
const unreachableRedisConfig = { PAIGASUS_SESSION_STORE: 'redis', PAIGASUS_SESSION_REDIS_URL: 'redis://127.0.0.1:1', PAIGASUS_SESSION_REDIS_TIMEOUT_MS: 50 } as ConsoleCoreConfig;

const silentLog = createJsonLogger(() => undefined);

afterEach(() => {
  resetDiscoveryForTest();
});

describe('the descriptor cache survives a module reload', () => {
  it('returns the same cache to two module instances', async () => {
    const first = descriptorCacheFor(memoryConfig);
    // A fresh query string defeats the module cache, which is what a dev recompile does.
    const reloaded = (await import(`../../src/discovery?reload=${String(Date.now())}`)) as typeof import('../../src/discovery');
    const second = reloaded.descriptorCacheFor(memoryConfig);
    expect(second).toBe(first);
  });

  it('still forgets the cache when the reset seam is called', () => {
    const first = descriptorCacheFor(memoryConfig);
    resetDiscoveryForTest();
    expect(descriptorCacheFor(memoryConfig)).not.toBe(first);
  });
});

describe('the reset seam (SMA-648 D10)', () => {
  // node-redis's destroy() THROWS ClientClosedError on a client that is already closed
  // (@redis/client 6.2.1, dist/lib/client/socket.js destroy()). A throw in the seam skips the two
  // lines that clear the state, so the next caller inherits a dead cache.
  it('does not throw when the Redis client is already closed, and still clears the state', () => {
    descriptorCacheFor(unreachableRedisConfig, silentLog);
    const client = redisClientFromState();
    expect(client?.isOpen).toBe(true);
    client?.destroy();
    expect(client?.isOpen).toBe(false);
    expect(() => {
      resetDiscoveryForTest();
    }).not.toThrow();
    expect(redisClientFromState()).toBeUndefined();
  });
});
```

- [ ] **Step 3: Run it and see it fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-console-core exec vitest run tests/unit/discovery-singleton.test.ts
```

Expected: the D10 test FAILS with `The client is closed` (a `ClientClosedError` thrown by
`resetDiscoveryForTest`). The `afterEach` also throws the same error for that test. The two
module-reload tests pass.

- [ ] **Step 4: Implement D10**

In `ts/packages/paigasus-console-core/src/discovery.ts`, replace lines 130-136:

```ts
/** Test and shutdown seam: forget the process cache and close the Redis client, if any. */
export function resetDiscoveryForTest(): void {
  const current = state();
  current.redisClient?.destroy();
  current.redisClient = undefined;
  current.processCache = undefined;
}
```

with:

```ts
/**
 * Test and shutdown seam: forget the process cache and close the Redis client, if any.
 *
 * It destroys the client ONLY while it is still open (SMA-648 D10). node-redis's destroy() throws
 * ClientClosedError on a client that is already closed (@redis/client socket.js destroy()), and a
 * throw here would skip the two lines that clear the state, so the next caller would inherit a
 * dead cache.
 */
export function resetDiscoveryForTest(): void {
  const current = state();
  if (current.redisClient?.isOpen === true) current.redisClient.destroy();
  current.redisClient = undefined;
  current.processCache = undefined;
}
```

- [ ] **Step 5: Run it and see it pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-console-core exec vitest run
pnpm -C ts/packages/paigasus-console-core exec tsc -p tsconfig.json --noEmit
```

Expected: all console-core unit and integration tests PASS (the three in
`discovery-singleton.test.ts` included). `tsc` exits 0.

- [ ] **Step 6: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write packages/paigasus-console-core/src/discovery.ts packages/paigasus-console-core/tests/support/discovery-state.ts packages/paigasus-console-core/tests/unit/discovery-singleton.test.ts
git add ts/packages/paigasus-console-core/src/discovery.ts ts/packages/paigasus-console-core/tests/support/discovery-state.ts ts/packages/paigasus-console-core/tests/unit/discovery-singleton.test.ts
git commit -F - <<'EOF'
fix(ts): reset the discovery state even when the Redis client is closed (SMA-648)

resetDiscoveryForTest destroys the client only while it is open. node-redis
throws ClientClosedError on a second destroy, and the throw skipped the lines
that clear the process state. This is spec decision D10, done before any red
run of the new container tier.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

## Task 2: The console-core Docker tier, with T1–T5 red against the unfixed client

**Files:**
- Modify: `ts/packages/paigasus-console-core/package.json` (devDependencies)
- Regenerate: `ts/pnpm-lock.yaml`
- Create: `ts/packages/paigasus-console-core/vitest.containers.config.ts`
- Modify: `ts/packages/paigasus-console-core/vitest.config.ts:34-38`
- Modify: `ts/packages/paigasus-console-core/tsconfig.json:9`
- Modify: `ts/packages/paigasus-console-core/moon.yml` (append after line 64)
- Create: `ts/packages/paigasus-console-core/README.md`
- Create: `ts/packages/paigasus-console-core/tests/containers/descriptor-cache-idle.test.ts`
- Modify: this plan file (append the red-run record at the end)

**Interfaces:**
- Consumes: `descriptorCacheFor(config, log)`, `resetDiscoveryForTest()`, `createJsonLogger(write)`, `DescriptorCache` (type, from `@paigasus/discovery/server`), `ConsoleCoreConfig`.
- Produces: Moon task `paigasus-console-core-ts:test-e2e`. Reads log lines by the event-name STRING `'discovery.redis_connection_lost'`, so the file compiles before that name joins `AppEventName`.

- [ ] **Step 1: Add the devDependency and install**

In `ts/packages/paigasus-console-core/package.json`, `devDependencies` becomes (insert
`testcontainers` between `react` and `typescript`):

```json
  "devDependencies": {
    "@connectrpc/connect-node": "catalog:",
    "@paigasus/proto": "workspace:*",
    "@types/node": "catalog:",
    "@types/react": "catalog:",
    "jose": "catalog:",
    "msw": "catalog:",
    "next": "catalog:",
    "react": "catalog:",
    "testcontainers": "catalog:",
    "typescript": "catalog:",
    "vitest": "catalog:"
  }
```

Then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts install
git diff --stat ts/pnpm-lock.yaml
```

Expected: `pnpm install` exits 0. The lockfile diff adds `testcontainers` (12.1.0, already in the
catalog as `^12.1.0`) to the `packages/paigasus-console-core` importer only. If the diff touches
anything else, stop and report it.

- [ ] **Step 2: Create the container-tier vitest config**

Write `ts/packages/paigasus-console-core/vitest.containers.config.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// The Docker-backed tier (SMA-648 § 4.4), run only by the `test-e2e` Moon task. A SEPARATE config,
// copied from @paigasus/discovery's vitest.containers.config.ts: this suite needs its own include,
// timeouts and `cache: false`, which a CLI path filter on the default config cannot supply.
//
// The `server-only` ALIAS is what makes the stub apply, not a setup file: src/discovery.ts and
// src/logger.ts both `import 'server-only'`, whose default export is an unconditional throw. The
// alias is the same one vitest.config.ts carries.
//
// There is NO SKIP HATCH when Docker is unreachable. This suite fails loudly, the precedent
// @paigasus/auth and @paigasus/discovery set.
const conditions = ['node', 'import', 'default'];
const serverOnlyStub = fileURLToPath(new URL('./tests/support/server-only-stub.ts', import.meta.url));

export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/containers/**/*.test.ts'],
    testTimeout: 120_000,
    hookTimeout: 300_000,
  },
  resolve: { conditions, alias: { 'server-only': serverOnlyStub } },
  ssr: { resolve: { conditions } },
});
```

- [ ] **Step 3: Exclude the containers from the default tier**

In `ts/packages/paigasus-console-core/vitest.config.ts`, replace lines 34-38:

```ts
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts', 'tests/**/*.test.tsx'],
    setupFiles: ['./tests/support/setup.ts'],
  },
```

with:

```ts
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts', 'tests/**/*.test.tsx'],
    // The container-backed suite lives under tests/containers/ and runs only in the test-e2e task
    // (vitest.containers.config.ts). Without this line the Docker-free `test` task would start it.
    exclude: ['tests/containers/**', '**/node_modules/**'],
    setupFiles: ['./tests/support/setup.ts'],
  },
```

- [ ] **Step 4: Put the new config in the TypeScript program**

In `ts/packages/paigasus-console-core/tsconfig.json`, replace line 9:

```json
  "include": ["src/**/*", "tests/**/*", "testing/**/*", "vitest.config.ts"]
```

with:

```json
  "include": ["src/**/*", "tests/**/*", "testing/**/*", "vitest.config.ts", "vitest.containers.config.ts"]
```

(`ts:lint` uses `projectService: true` with no default project. A `.ts` file in no program fails
lint.)

- [ ] **Step 5: Add the Moon task**

Append to `ts/packages/paigasus-console-core/moon.yml`, after line 64 (the last `test` input),
at the same indentation as `test:`:

```yaml
  test-e2e:
    # SMA-648 § 4.4: the Docker-backed tier. It drives the REAL call site, descriptorCacheFor, against
    # a real Redis, because the defect lived in the client options that call site passes.
    # `script` (not `command`): Moon's `command` setting rejects shell syntax. `set -euo pipefail`
    # because Moon does not enable errexit for script blocks.
    # NO SKIP HATCH: like @paigasus/auth's and @paigasus/discovery's tiers, this task fails when
    # Docker is unreachable.
    script: |
      set -euo pipefail
      pnpm exec vitest run --config vitest.containers.config.ts
    # src/discovery.ts loads @paigasus/discovery/server at RUN time, whose core imports
    # @paigasus/proto's generated tree. contracts:generate deletes that tree before it rewrites it,
    # so this task must be ordered after it (the measured failure is in
    # ts/packages/paigasus-discovery/moon.yml).
    deps: ['contracts:generate']
    # Inputs are the ONLY thing that confers affectedness on Moon 2.5.3. This list covers this
    # package's source and tests, both vitest configs, tsconfig.json, package.json, the lockfile,
    # and @paigasus/discovery's source (which owns createRedisDescriptorCache's preconditions).
    # It deliberately does NOT list testing/**/*: this tier imports nothing from testing/.
    inputs:
      - 'src/**/*'
      - 'tests/**/*'
      - 'vitest.config.ts'
      - 'vitest.containers.config.ts'
      - 'tsconfig.json'
      - 'package.json'
      - '/ts/pnpm-lock.yaml'
      - '/ts/packages/paigasus-discovery/src/**/*'
      - '/ts/packages/paigasus-discovery/package.json'
    options:
      # A real container. A cached PASS would replay a green that tested nothing.
      cache: false
```

Verify that Moon parses it:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query tasks | python3 -c 'import sys, json; d = json.load(sys.stdin); print(sorted(d["tasks"]["paigasus-console-core-ts"]))'
```

Expected: `['build', 'test', 'test-e2e', 'typecheck']`.

- [ ] **Step 6: Create the package README**

The package has no README today. Write `ts/packages/paigasus-console-core/README.md`:

```markdown
# @paigasus/console-core

Server-only console composition shared by the Paigasus console zones: the IAM
clients, provisioning and the principal resolver, the authorization
self-queries, scopes, discovery, the error wrapper and the JSON-lines logger.

The root export is server-only. The `./testing` subpath holds in-process fakes
for test harnesses and the `dev:stack` command.

## Tests

- `moon run paigasus-console-core-ts:test` runs the unit and integration tests.
  It needs no Docker.
- `moon run paigasus-console-core-ts:test-e2e` runs the Docker-backed tier in
  `tests/containers/`. It starts a real Redis with testcontainers and drives the
  descriptor cache's Redis client through idle gaps, a server pause and a
  server-side close (SMA-648).

The `test-e2e` task **needs Docker** and has **no skip hatch**. It fails when
Docker is unreachable. Its inputs include this package's `src/**/*`, so an edit
to any console-core source file now selects a task that needs Docker. It also
selects on `@paigasus/discovery`'s `src/**/*`.
```

- [ ] **Step 7: Write the container tests**

Write `ts/packages/paigasus-console-core/tests/containers/descriptor-cache-idle.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-648 § 5.1: the descriptor cache's Redis client must survive an idle socket. This drives the
// REAL call site, descriptorCacheFor, against a real Redis, because the defect lived in the client
// options that call site passes to node-redis.
//
// PAIGASUS_SESSION_REDIS_TIMEOUT_MS is 500, so `socketTimeout` is 1000 ms and `pingInterval` is
// 500 ms. Every poll is bounded by POLL_BUDGET_MS. Each test has its own log sink, so a late event
// from the previous test's destroy() cannot land in the next test's lines.
//
// The Redis carries a password (`--requirepass`), so every URL in this file holds a secret, and T5
// can assert that no log line ever contains it.
//
// Log lines are read by the EVENT NAME STRING, not through the AppEventName type. So this file
// compiles and runs against code that does not have `discovery.redis_connection_lost` yet, which
// the red-first run (spec § 5.1) needs.
import { randomBytes } from 'node:crypto';
import { setTimeout as sleep } from 'node:timers/promises';
import { GenericContainer, type StartedTestContainer } from 'testcontainers';
import { createClient, type RedisClientType } from 'redis';
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest';
import type { DescriptorCache } from '@paigasus/discovery/server';
import type { ConsoleCoreConfig } from '../../src/config-shape';
import { descriptorCacheFor, resetDiscoveryForTest } from '../../src/discovery';
import { createJsonLogger, type ConsoleLogger } from '../../src/logger';

const TIMEOUT_MS = 500;
const SOCKET_TIMEOUT_MS = TIMEOUT_MS * 2;
/** Three times socketTimeout. */
const IDLE_MS = 3 * SOCKET_TIMEOUT_MS;
/** More than pingInterval + 2 × socketTimeout = 2500 ms, so the idle timer MUST fire during it. */
const PAUSE_MS = 3_000;
const POLL_BUDGET_MS = 5_000;
const POLL_STEP_MS = 100;
const LOST_EVENT = 'discovery.redis_connection_lost';
const SECRET = `s3cret${randomBytes(8).toString('hex')}`;

type CacheRecord = Parameters<DescriptorCache['set']>[1];
type LogLine = { readonly event: string; readonly fields: Readonly<Record<string, unknown>> };

let container: StartedTestContainer;
let url: string;
let admin: RedisClientType;

beforeAll(async () => {
  container = await new GenericContainer('redis:8-alpine').withCommand(['redis-server', '--requirepass', SECRET]).withExposedPorts(6379).start();
  url = `redis://:${SECRET}@${container.getHost()}:${container.getMappedPort(6379)}`;
  // A SECOND client, never the cache's own: it pauses the server and kills the cache's connection.
  admin = createClient({ url });
  // Never log the raw error: node-redis embeds the DSN in its connection errors.
  admin.on('error', () => undefined);
  await admin.connect();
});

afterEach(() => {
  resetDiscoveryForTest();
});

afterAll(async () => {
  if (admin?.isOpen) admin.destroy();
  await container?.stop();
});

function redisConfig(): ConsoleCoreConfig {
  return { PAIGASUS_SESSION_STORE: 'redis', PAIGASUS_SESSION_REDIS_URL: url, PAIGASUS_SESSION_REDIS_TIMEOUT_MS: TIMEOUT_MS } as ConsoleCoreConfig;
}

function record(): CacheRecord {
  return { version: 1, rev: 1, descriptor: { service: 'iam', version: '1.0.0', capabilities: ['iam.audit'] }, descriptorAt: 0, outcome: 'ok', outcomeAt: 0, reason: null };
}

/** One sink per test. `lost()` returns the fields of every connection-lost line, in order. */
function logSink(): { log: ConsoleLogger; lines: string[]; lost: () => Array<LogLine['fields']> } {
  const lines: string[] = [];
  const log = createJsonLogger((line) => {
    lines.push(line);
  });
  const lost = (): Array<LogLine['fields']> =>
    lines
      .map((line) => JSON.parse(line) as LogLine)
      .filter((line) => line.event === LOST_EVENT)
      .map((line) => line.fields);
  return { log, lines, lost };
}

/** Retries `attempt` every POLL_STEP_MS until it resolves, for at most POLL_BUDGET_MS. */
async function eventually<T>(attempt: () => Promise<T>, what: string): Promise<T> {
  const deadline = Date.now() + POLL_BUDGET_MS;
  let lastError: unknown;
  for (;;) {
    try {
      return await attempt();
    } catch (error) {
      lastError = error;
    }
    if (Date.now() >= deadline) {
      throw new Error(`${what}: no success within ${POLL_BUDGET_MS} ms; the last error was "${lastError instanceof Error ? lastError.message : 'not an Error'}"`);
    }
    await sleep(POLL_STEP_MS);
  }
}

/** Makes the process cache and waits (bounded) until a first write succeeds, before any idle gap. */
async function warmCache(log: ConsoleLogger): Promise<{ cache: DescriptorCache; service: string }> {
  const cache = descriptorCacheFor(redisConfig(), log);
  const service = `idle-${randomBytes(4).toString('hex')}`;
  await eventually(() => cache.set(service, record(), 60_000, null), 'the first set');
  return { cache, service };
}

/** Kills every normal connection except the admin client's own, found by `CLIENT ID`. */
async function killCacheConnections(): Promise<number> {
  const adminId = await admin.clientId();
  const others = (await admin.clientList({ TYPE: 'NORMAL' })).filter((client) => client.id !== adminId);
  for (const client of others) {
    await admin.clientKill({ filter: 'ID', id: client.id });
  }
  return others.length;
}

describe('the descriptor cache Redis client (SMA-648)', () => {
  it('T1: a read after an idle gap past socketTimeout returns the record, and no loss is logged', async () => {
    const sink = logSink();
    const { cache, service } = await warmCache(sink.log);
    await sleep(IDLE_MS);
    expect(await cache.get(service)).toEqual(record());
    expect(sink.lost()).toEqual([]);
  });

  it('T2: a read after each of two idle gaps returns the record', async () => {
    const sink = logSink();
    const { cache, service } = await warmCache(sink.log);
    await sleep(IDLE_MS);
    expect(await cache.get(service)).toEqual(record());
    await sleep(IDLE_MS);
    expect(await cache.get(service)).toEqual(record());
    expect(sink.lost()).toEqual([]);
  });

  it('T3: a real hang (CLIENT PAUSE) fires the idle timer, the client recovers, and one socket_timeout loss is logged', async () => {
    const sink = logSink();
    const { cache, service } = await warmCache(sink.log);
    // The cache's next PING is in flight with no reply, and nothing else writes to its socket, so
    // the idle timer fires. Nothing in this test touches the cache until the pause is over.
    await admin.clientPause(PAUSE_MS, 'ALL');
    await sleep(PAUSE_MS + 200);
    expect(await eventually(() => cache.get(service), 'a read after the pause')).toEqual(record());
    expect(sink.lost()).toEqual([{ reason: 'socket_timeout' }]);
  });

  it('T4: a server-side close (CLIENT KILL) recovers, and one socket_closed loss is logged', async () => {
    const sink = logSink();
    const { cache, service } = await warmCache(sink.log);
    expect(await killCacheConnections()).toBeGreaterThan(0);
    expect(await eventually(() => cache.get(service), 'a read after the server-side close')).toEqual(record());
    expect(sink.lost()).toEqual([{ reason: 'socket_closed' }]);
  });

  it('T5: no log line holds the password from the DSN', async () => {
    const sink = logSink();
    const { cache, service } = await warmCache(sink.log);
    expect(await killCacheConnections()).toBeGreaterThan(0);
    await eventually(() => cache.get(service), 'a read after the server-side close');
    // A guard, exempt from red-first: it must see at least one line, or it proves nothing.
    expect(sink.lost().length).toBeGreaterThan(0);
    for (const line of sink.lines) expect(line).not.toContain(SECRET);
  });
});
```

- [ ] **Step 8: Type-check and lint the new files**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-console-core exec tsc -p tsconfig.json --noEmit
pnpm -C ts exec eslint packages/paigasus-console-core
pnpm -C ts exec prettier --write packages/paigasus-console-core/package.json packages/paigasus-console-core/vitest.config.ts packages/paigasus-console-core/vitest.containers.config.ts packages/paigasus-console-core/tsconfig.json packages/paigasus-console-core/moon.yml packages/paigasus-console-core/README.md packages/paigasus-console-core/tests/containers/descriptor-cache-idle.test.ts
```

Expected: `tsc` exits 0, `eslint` exits 0. Fix any finding in the test file before you go on.

- [ ] **Step 9: Run the default tier (no Docker)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-console-core exec vitest run
```

Expected: PASS, and `tests/containers/descriptor-cache-idle.test.ts` is NOT in the file list.

- [ ] **Step 10: Run the container tier and see T1–T5 fail for the stated reasons**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-console-core exec vitest run --config vitest.containers.config.ts 2>&1 | tee "${TMPDIR:-/tmp}/sma648-red-run.txt"
```

Expected (spec § 5.1 table). Each failure must be for THIS reason, not for another:

- T1 FAILS: `cache.get` rejects with `The client is closed`.
- T2 FAILS: the first `cache.get` rejects with `The client is closed`.
- T3 FAILS: `a read after the pause: no success within 5000 ms; the last error was "The client is closed"`.
- T4 FAILS on the LOG check only: the read succeeds, and `expect(sink.lost())` shows `[]` against `[{ reason: 'socket_closed' }]`.
- T5 FAILS on `expect(sink.lost().length).toBeGreaterThan(0)` (it is a guard, exempt from red-first).

If a test fails for any other reason (for example the container does not start, or T1 fails in
`the first set`), stop. Fix the harness, not the assertion, and run again.

- [ ] **Step 11: Record the red run in this plan**

Append this section to the END of this plan file, and fill each line with the actual first
failure line from `${TMPDIR:-/tmp}/sma648-red-run.txt` (copy the assertion line, not the stack):

```markdown
## Red-run record (Task 2, unmodified client after D10)

- T1: <actual failure line>
- T2: <actual failure line>
- T3: <actual failure line>
- T4: <actual failure line>
- T5: <actual failure line>
```

(These angle-bracket slots are for the implementer to fill with measured output. They are not
plan placeholders.)

- [ ] **Step 12: Commit**

```bash
git add ts/packages/paigasus-console-core/package.json ts/pnpm-lock.yaml ts/packages/paigasus-console-core/vitest.containers.config.ts ts/packages/paigasus-console-core/vitest.config.ts ts/packages/paigasus-console-core/tsconfig.json ts/packages/paigasus-console-core/moon.yml ts/packages/paigasus-console-core/README.md ts/packages/paigasus-console-core/tests/containers/descriptor-cache-idle.test.ts docs/superpowers/plans/2026-09-18-sma-648-descriptor-cache-redis-idle.md
git commit -F - <<'EOF'
test(ts): a Docker tier for the descriptor cache Redis client (SMA-648)

A new test-e2e task in @paigasus/console-core starts a real Redis and drives
descriptorCacheFor through two idle gaps, a CLIENT PAUSE and a CLIENT KILL.
T1 to T4 fail on the unfixed client for the reasons the spec states. T5 is a
guard. The red run is recorded in the plan. The task has no skip hatch.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

## Task 3: The fix in console-core (D2, D3, D6, D7, D8, D11)

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/logger.ts:14`
- Modify: `ts/packages/paigasus-console-core/src/discovery.ts` (whole file shown in Step 4)
- Create: `ts/packages/paigasus-console-core/tests/unit/discovery-redis-client.test.ts`

**Interfaces:**
- Consumes: `SocketTimeoutError`, `SocketClosedUnexpectedlyError`, `createClient`, `RedisClientType` from `'redis'`; `redisClientFromState()` from Task 1.
- Produces (all exported from `src/discovery.ts`, NOT added to `src/index.ts`):
  - `export function descriptorCacheReconnectStrategy(retries: number): number`
  - `export type ConnectionLossReason = 'socket_timeout' | 'socket_closed' | 'other'`
  - `export type ConnectionLossSource = { readonly isOpen: boolean; readonly isReady: boolean; on(event: 'ready' | 'error', listener: (error: unknown) => void): unknown }`
  - `export function watchConnectionLoss(client: ConnectionLossSource, log: ConsoleLogger): void`
  - `AppEventName` gains `'discovery.redis_connection_lost'`; the event's fields are exactly `{ reason: ConnectionLossReason }`.

- [ ] **Step 1: Write the failing default-tier tests**

Write `ts/packages/paigasus-console-core/tests/unit/discovery-redis-client.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-648 § 5.2, the default tier (no Docker): the reconnect strategy, the connection-loss logger
// against a fake emitter, and a pin on the client options descriptorCacheFor passes to node-redis.
import { EventEmitter } from 'node:events';
import { SocketClosedUnexpectedlyError, SocketTimeoutDuringMaintenanceError, SocketTimeoutError } from 'redis';
import { afterEach, describe, expect, it } from 'vitest';
import type { ConsoleCoreConfig } from '../../src/config-shape';
import { descriptorCacheFor, descriptorCacheReconnectStrategy, resetDiscoveryForTest, watchConnectionLoss } from '../../src/discovery';
import { createJsonLogger } from '../../src/logger';
import { redisClientFromState } from '../support/discovery-state';

afterEach(() => {
  resetDiscoveryForTest();
});

describe('descriptorCacheReconnectStrategy (D3)', () => {
  it('returns a finite delay in [0, 2199] ms for every retry count from 0 to 20', () => {
    for (let retries = 0; retries <= 20; retries += 1) {
      // The jitter is random, so sample each retry count many times.
      for (let sample = 0; sample < 50; sample += 1) {
        const delay = descriptorCacheReconnectStrategy(retries);
        expect(Number.isFinite(delay)).toBe(true);
        expect(delay).toBeGreaterThanOrEqual(0);
        expect(delay).toBeLessThanOrEqual(2199);
      }
    }
  });

  it('returns a delay, never false or an Error, when node-redis passes a SocketTimeoutError', () => {
    // node-redis calls the strategy as (retries, cause). Its DEFAULT strategy returns false here.
    const asNodeRedisCallsIt = descriptorCacheReconnectStrategy as (retries: number, cause: Error) => unknown;
    expect(typeof asNodeRedisCallsIt(0, new SocketTimeoutError(1000))).toBe('number');
  });
});

/** A fake node-redis client: an EventEmitter with settable isOpen / isReady. */
class FakeClient extends EventEmitter {
  isOpen = true;
  isReady = false;
}

function watched(): { client: FakeClient; lines: string[]; lost: () => unknown[]; becomeReady: () => void; loseSocket: (error: unknown) => void } {
  const lines: string[] = [];
  const client = new FakeClient();
  watchConnectionLoss(
    client,
    createJsonLogger((line) => {
      lines.push(line);
    }),
  );
  const lost = (): unknown[] =>
    lines
      .map((line) => JSON.parse(line) as { event: string; fields: unknown })
      .filter((line) => line.event === 'discovery.redis_connection_lost')
      .map((line) => line.fields);
  const becomeReady = (): void => {
    client.isReady = true;
    client.emit('ready');
  };
  // node-redis's #onSocketError sets isReady = false BEFORE it emits, and isOpen stays true.
  const loseSocket = (error: unknown): void => {
    client.isReady = false;
    client.emit('error', error);
  };
  return { client, lines, lost, becomeReady, loseSocket };
}

describe('watchConnectionLoss (D6, D7, D8)', () => {
  it('registers exactly one error listener, which createRedisDescriptorCache requires', () => {
    const { client } = watched();
    expect(client.listenerCount('error')).toBe(1);
  });

  it('logs nothing for an error before the first ready (a connect failure)', () => {
    const { client, lines } = watched();
    client.emit('error', new Error('connect ECONNREFUSED'));
    expect(lines).toEqual([]);
  });

  it('logs nothing for an error while the socket stays ready (an error reply, e.g. -NOPERM to a PING)', () => {
    const { client, lines, becomeReady } = watched();
    becomeReady();
    client.emit('error', new Error("NOPERM this user has no permissions to run the 'ping' command"));
    expect(lines).toEqual([]);
  });

  it('logs nothing for an error after destroy() (isOpen false)', () => {
    const { client, lines, becomeReady } = watched();
    becomeReady();
    client.isOpen = false;
    client.isReady = false;
    client.emit('error', new Error('Disconnects client'));
    expect(lines).toEqual([]);
  });

  it('logs one line for a socket loss (isOpen true, isReady false)', () => {
    const { lost, becomeReady, loseSocket } = watched();
    becomeReady();
    loseSocket(new SocketTimeoutError(1000));
    expect(lost()).toEqual([{ reason: 'socket_timeout' }]);
  });

  it('logs no more lines for the handshake errors of the same reconnect loop', () => {
    const { client, lost, becomeReady, loseSocket } = watched();
    becomeReady();
    loseSocket(new SocketTimeoutError(1000));
    client.emit('error', new SocketTimeoutError(1000));
    client.emit('error', new Error('connect ECONNREFUSED'));
    expect(lost()).toEqual([{ reason: 'socket_timeout' }]);
  });

  it('logs one more line for a second loss after a new ready', () => {
    const { lost, becomeReady, loseSocket } = watched();
    becomeReady();
    loseSocket(new SocketTimeoutError(1000));
    becomeReady();
    loseSocket(new SocketClosedUnexpectedlyError());
    expect(lost()).toEqual([{ reason: 'socket_timeout' }, { reason: 'socket_closed' }]);
  });

  it.each([
    { error: new SocketTimeoutError(1000), reason: 'socket_timeout' },
    { error: new SocketClosedUnexpectedlyError(), reason: 'socket_closed' },
    // Redis Enterprise maintenance only. NOT a subclass of SocketTimeoutError (D7), so it is `other`.
    { error: new SocketTimeoutDuringMaintenanceError(1000), reason: 'other' },
    { error: new Error('connect ECONNREFUSED redis://:s3cret-password@redis.internal:6379'), reason: 'other' },
  ])('maps $error.constructor.name to reason "$reason", and never logs the message', ({ error, reason }) => {
    const { lines, lost, becomeReady, loseSocket } = watched();
    becomeReady();
    loseSocket(error);
    expect(lost()).toEqual([{ reason }]);
    expect(lines.join('\n')).not.toContain('s3cret-password');
    expect(lines.join('\n')).not.toContain(error.message);
  });
});

describe('the client options descriptorCacheFor passes to node-redis (D2, D3)', () => {
  it('sets a ping of at most half the idle timer, and the reconnect strategy', () => {
    // Port 1 refuses the connection. The options are fixed when the client is made.
    descriptorCacheFor(
      { PAIGASUS_SESSION_STORE: 'redis', PAIGASUS_SESSION_REDIS_URL: 'redis://127.0.0.1:1', PAIGASUS_SESSION_REDIS_TIMEOUT_MS: 500 } as ConsoleCoreConfig,
      createJsonLogger(() => undefined),
    );
    const options = redisClientFromState()?.options;
    const pingInterval = options?.pingInterval;
    const socketTimeout = options?.socket?.socketTimeout;
    expect(pingInterval).toBeTypeOf('number');
    expect(socketTimeout).toBeTypeOf('number');
    expect(pingInterval as number).toBeGreaterThan(0);
    expect(pingInterval as number).toBeLessThanOrEqual((socketTimeout as number) / 2);
    expect(options?.socket?.reconnectStrategy).toBe(descriptorCacheReconnectStrategy);
  });
});
```

- [ ] **Step 2: Run it and see it fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-console-core exec vitest run tests/unit/discovery-redis-client.test.ts
```

Expected: FAIL. `descriptorCacheReconnectStrategy` and `watchConnectionLoss` are not exported, so
the tests fail with `descriptorCacheReconnectStrategy is not a function` /
`watchConnectionLoss is not a function`, and the options pin sees `pingInterval` `undefined`.

- [ ] **Step 3: Add the event name to the logger**

In `ts/packages/paigasus-console-core/src/logger.ts`, replace line 14:

```ts
export type AppEventName = 'principal.resolve_failed' | 'principal.resolve_crashed' | 'authorize.query_failed' | 'authorize.no_principal' | 'discovery.redis_connect_failed' | 'iam.call_failed';
```

with:

```ts
export type AppEventName =
  | 'principal.resolve_failed'
  | 'principal.resolve_crashed'
  | 'authorize.query_failed'
  | 'authorize.no_principal'
  | 'discovery.redis_connect_failed'
  | 'discovery.redis_connection_lost'
  | 'iam.call_failed';
```

(Prettier may put it back on one line; accept what `prettier --write` produces.)

- [ ] **Step 4: Implement the fix in discovery.ts**

Replace the whole of `ts/packages/paigasus-console-core/src/discovery.ts` with the file below.
The `DiscoveryState` block, `afterConnect`, `descriptorCacheFor`, `resetDiscoveryForTest` (as
Task 1 left it) and `createAppDiscovery` are unchanged; they are repeated so the file is
complete. What changes: the header comment, the `redis` import, the new exports above
`connectOnce`, `connectOnce`'s rejection handler, and `redisDescriptorCache`.

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Capability discovery for this app (spec § 4.4). ONE Discovery handle PER REQUEST (React cache()),
// as @paigasus/discovery requires (ts/packages/paigasus-discovery/src/server.ts:63-82), over a
// descriptor cache that is a PROCESS singleton.
//
//   PAIGASUS_SESSION_STORE=redis  -> the SAME Redis URL as the session store, so an operator
//                                    configures one Redis. A lazy node-redis client, made on first
//                                    use with the preconditions createRedisDescriptorCache asserts
//                                    (ts/packages/paigasus-discovery/src/adapters/redis-cache.ts).
//   PAIGASUS_SESSION_STORE=memory -> the memory cache.
//
// There is NO silent fallback from Redis to memory. When Redis cannot connect, the cache fails
// fast, discovery reports its own `cache-unavailable` degraded reason, and this file logs
// `discovery.redis_connect_failed` once, with no DSN.
//
// THE REDIS CLIENT MUST SURVIVE AN IDLE SOCKET (SMA-648). Read from @redis/client 6.2.1:
//
// - `socket.socketTimeout` (2 × PAIGASUS_SESSION_REDIS_TIMEOUT_MS) is an IDLE timer, not a reply
//   deadline. node-redis calls Node's socket.setTimeout, which fires after that many ms with no
//   socket activity, and any read OR write resets it. So it fires on a quiet, healthy connection.
//   It bounds a command in flight only while nothing else is written to the socket: under steady
//   traffic each write moves the deadline, and a Redis that accepts commands and never replies can
//   go unnoticed (follow-up SMA-650).
// - `pingInterval` (PAIGASUS_SESSION_REDIS_TIMEOUT_MS) sends PING while the socket is ready. The
//   PING and its reply are socket activity, so a healthy idle socket never reaches the idle timer.
//   The gap between "the ping is due" and "the idle timer fires" is one timeout. That gap is the
//   tolerance for event-loop lag.
// - The PING RATE depends on PAIGASUS_SESSION_REDIS_TIMEOUT_MS: one PING per timeout, per process.
//   A value of 1 gives one PING per ms. There is deliberately no floor: a floor would break the
//   `pingInterval <= socketTimeout / 2` precondition for small values (spec § 3).
// - node-redis's DEFAULT reconnect strategy returns `false` for a SocketTimeoutError, which closes
//   the client for the life of the process. descriptorCacheReconnectStrategy returns a delay for
//   EVERY cause and has no retry cap, for the reason @paigasus/auth's reconnectStrategy records:
//   one client, no recreate path, so a stop is permanent.
//
// The Redis user needs the `+ping` ACL permission. Without it every PING gets a `-NOPERM` reply.
// That reply is still socket activity, so the socket stays alive, and watchConnectionLoss ignores
// the error (the socket stays ready), so nothing reports the missing permission. Grant it.
import 'server-only';
import { createClient, SocketClosedUnexpectedlyError, SocketTimeoutError, type RedisClientType } from 'redis';
import { createDiscovery, createMemoryDescriptorCache, createRedisDescriptorCache, timingsFromEnv, type DescriptorCache, type Discovery } from '@paigasus/discovery/server';
import type { ConsoleCoreConfig } from './config-shape';
import { logger, type ConsoleLogger } from './logger';

// MODULE scope is not enough. `next dev` recompiles the module graph on every edit, so a
// module-level singleton is remade each time and its Redis client leaks — one per recompile, for
// the life of a dev session. @paigasus/auth's getAuthRuntime carries the same fix for the same
// reason (src/runtime.ts:171-230): hold the state on globalThis, under a key that survives a
// reload. `Symbol.for` resolves through a registry that is GLOBAL to the process, keyed by the
// string given to it — every module that calls `Symbol.for` with the same string gets the SAME
// symbol back, so the key must be specific to this cache AND carry a version segment, the same
// reasoning `runtime.ts`'s `RUNTIME_KEY_PREFIX` states for its own key. The key carries no zone
// segment, and that is deliberate: `descriptorCacheFor` is documented above as a process-wide
// singleton, so sharing one cache across every zone composed into this process is the wanted
// behaviour, not the bug the zone segment in `runtime.ts`'s key exists to avoid.
type DiscoveryState = { processCache?: DescriptorCache | undefined; redisClient?: RedisClientType | undefined };

const DISCOVERY_STATE = Symbol.for('paigasus.console-core.discovery-state.v1');

function state(): DiscoveryState {
  const holder = globalThis as typeof globalThis & { [DISCOVERY_STATE]?: DiscoveryState };
  holder[DISCOVERY_STATE] ??= {};
  return holder[DISCOVERY_STATE];
}

const MAX_RECONNECT_DELAY_MS = 2000;
const MAX_RECONNECT_JITTER_MS = 200;

/**
 * The descriptor cache's reconnect strategy (SMA-648 D3). It returns a delay for EVERY cause,
 * SocketTimeoutError included, and never `false` or an Error: `min(2^retries × 50, 2000)` ms plus
 * 0–199 ms of jitter. There is no retry cap (see the file header). node-redis starts the FIRST
 * reconnect at once and uses this value only as a type check (@redis/client socket.js
 * #onSocketError), so the delay applies from the second attempt.
 */
export function descriptorCacheReconnectStrategy(retries: number): number {
  const jitter = Math.floor(Math.random() * MAX_RECONNECT_JITTER_MS);
  return Math.min(2 ** retries * 50, MAX_RECONNECT_DELAY_MS) + jitter;
}

/** The fixed `reason` values of `discovery.redis_connection_lost` (SMA-648 D7). */
export type ConnectionLossReason = 'socket_timeout' | 'socket_closed' | 'other';

/**
 * The part of a node-redis client that watchConnectionLoss reads. Structural, so the default test
 * tier can drive it with a plain EventEmitter.
 */
export type ConnectionLossSource = {
  readonly isOpen: boolean;
  readonly isReady: boolean;
  on(event: 'ready' | 'error', listener: (error: unknown) => void): unknown;
};

/**
 * A fixed value from `instanceof` checks, never the message or `constructor.name` (SMA-648 D7). A
 * node-redis error message can embed the DSN, the error classes do not set `name`, and a
 * production bundle can mangle `constructor.name`. SocketTimeoutDuringMaintenanceError is not a
 * subclass of SocketTimeoutError, so it maps to `other`.
 */
function connectionLossReason(error: unknown): ConnectionLossReason {
  if (error instanceof SocketTimeoutError) return 'socket_timeout';
  if (error instanceof SocketClosedUnexpectedlyError) return 'socket_closed';
  return 'other';
}

/**
 * Registers the client's `ready` and `error` listeners (SMA-648 D6, D8). The `error` listener is
 * the one createRedisDescriptorCache REQUIRES, and it never reads the error message: node-redis
 * embeds the DSN in its connection errors.
 *
 * It logs `discovery.redis_connection_lost` ONCE PER LOSS and ONLY FOR A REAL LOSS: when it is
 * armed, `client.isOpen`, and `!client.isReady`. Then it disarms. A `ready` event arms it again.
 * node-redis emits `error` for events that do NOT drop the socket (an error reply to a PING, a
 * decoder error, a PING that destroy() flushes); in those the socket stays ready, or the client is
 * no longer open. A real loss passes #onSocketError, which sets isReady = false BEFORE it emits,
 * and under descriptorCacheReconnectStrategy isOpen stays true. Errors before the first `ready`
 * are connect failures, which connectOnce reports. Handshake errors inside a reconnect loop arrive
 * while it is disarmed.
 */
export function watchConnectionLoss(client: ConnectionLossSource, log: ConsoleLogger): void {
  let armed = false;
  client.on('ready', () => {
    armed = true;
  });
  client.on('error', (error: unknown) => {
    if (!armed || !client.isOpen || client.isReady) return;
    armed = false;
    log.appEvent('discovery.redis_connection_lost', { reason: connectionLossReason(error) });
  });
}

/**
 * Waits for the first connect, but never longer than one command timeout. node-redis's connect()
 * keeps retrying while its reconnect strategy returns a delay (@redis/client socket.js #connect),
 * so an unreachable Redis would otherwise hold every render. The connect keeps running in the
 * background, so the cache recovers when Redis does.
 *
 * The rejection handler logs NOTHING (SMA-648 D11). Under descriptorCacheReconnectStrategy,
 * connect() rejects only when destroy() runs during a connect, and "connect failed" is the wrong
 * line for that. The handler stays so that a rejected connect() is never an unhandled rejection.
 * The `connect_timeout` line still covers an unreachable Redis.
 */
function connectOnce(client: RedisClientType, timeoutMs: number, log: ConsoleLogger): Promise<void> {
  let settled = false;
  const connected = client.connect().then(
    () => {
      settled = true;
    },
    () => {
      settled = true;
    },
  );
  const bounded = new Promise<void>((resolve) => {
    setTimeout(() => {
      if (!settled) log.appEvent('discovery.redis_connect_failed', { stage: 'connect_timeout' });
      resolve();
    }, timeoutMs).unref();
  });
  return Promise.race([connected, bounded]);
}

/** Wraps a cache so that every operation first waits (boundedly) for the first connect. */
function afterConnect(inner: DescriptorCache, ready: Promise<void>): DescriptorCache {
  return {
    get: async (service) => {
      await ready;
      return inner.get(service);
    },
    set: async (service, rec, ttlMs, expectedRev) => {
      await ready;
      return inner.set(service, rec, ttlMs, expectedRev);
    },
    delete: async (service) => {
      await ready;
      return inner.delete(service);
    },
    tryAcquireLock: async (service, token, ttlMs) => {
      await ready;
      return inner.tryAcquireLock(service, token, ttlMs);
    },
    releaseLock: async (service, token) => {
      await ready;
      return inner.releaseLock(service, token);
    },
    close: () => inner.close(),
  };
}

function redisDescriptorCache(url: string, timeoutMs: number, log: ConsoleLogger): DescriptorCache {
  const client: RedisClientType = createClient({
    url,
    disableOfflineQueue: true,
    commandOptions: { timeout: timeoutMs },
    // D2: a PING every timeout keeps a healthy idle socket under the idle timer (file header).
    pingInterval: timeoutMs,
    // D1: the idle timer stays; it is the only bound on a hung command. D3: the strategy reopens
    // the socket after the timer fires on a real hang.
    socket: { socketTimeout: timeoutMs * 2, connectTimeout: timeoutMs, reconnectStrategy: descriptorCacheReconnectStrategy },
  });
  // Registers the error listener createRedisDescriptorCache requires. It never logs the error.
  watchConnectionLoss(client, log);
  state().redisClient = client;
  const inner = createRedisDescriptorCache(client);
  return afterConnect(inner, connectOnce(client, timeoutMs, log));
}

/**
 * The process-wide descriptor cache for this configuration.
 *
 * NO SILENT FALLBACK (see the file header), so `redis` with no URL THROWS. That pair reaches this
 * function: the app's flat zod config shape cannot express the cross-field rule, and
 * `createAuthRuntime` — which does hold that rule — may not have run yet for this request. The old
 * code read the pair as "use memory", which is the exact silent downgrade the header forbids: every
 * zone would then cache descriptors in its own process and AC 2 would not hold. The message names
 * the two variables and never the URL, which carries a password.
 */
export function descriptorCacheFor(config: ConsoleCoreConfig, log: ConsoleLogger = logger): DescriptorCache {
  const current = state();
  if (current.processCache !== undefined) return current.processCache;
  if (config.PAIGASUS_SESSION_STORE === 'redis') {
    if (config.PAIGASUS_SESSION_REDIS_URL === undefined) {
      throw new Error('PAIGASUS_SESSION_REDIS_URL is required when PAIGASUS_SESSION_STORE is "redis"');
    }
    current.processCache = redisDescriptorCache(config.PAIGASUS_SESSION_REDIS_URL, config.PAIGASUS_SESSION_REDIS_TIMEOUT_MS, log);
  } else {
    current.processCache = createMemoryDescriptorCache();
  }
  return current.processCache;
}

/**
 * Test and shutdown seam: forget the process cache and close the Redis client, if any.
 *
 * It destroys the client ONLY while it is still open (SMA-648 D10). node-redis's destroy() throws
 * ClientClosedError on a client that is already closed (@redis/client socket.js destroy()), and a
 * throw here would skip the two lines that clear the state, so the next caller would inherit a
 * dead cache.
 */
export function resetDiscoveryForTest(): void {
  const current = state();
  if (current.redisClient?.isOpen === true) current.redisClient.destroy();
  current.redisClient = undefined;
  current.processCache = undefined;
}

/** A Discovery handle for one request. Pure apart from the process cache: tests call it directly. */
export function createAppDiscovery(deps: { config: ConsoleCoreConfig; log?: ConsoleLogger; fetch?: typeof globalThis.fetch; waitUntil?: (p: Promise<unknown>) => void }): Discovery {
  const log = deps.log ?? logger;
  return createDiscovery({
    services: deps.config.PAIGASUS_SERVICES,
    cache: descriptorCacheFor(deps.config, log),
    logger: log,
    timings: timingsFromEnv(deps.config),
    ...(deps.fetch === undefined ? {} : { fetch: deps.fetch }),
    ...(deps.waitUntil === undefined ? {} : { waitUntil: deps.waitUntil }),
  });
}
```

Do NOT add the new exports to `src/index.ts`. The spec exports them from `discovery.ts` only.

- [ ] **Step 5: Run the default tier and see it pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-console-core exec vitest run
pnpm -C ts/packages/paigasus-console-core exec tsc -p tsconfig.json --noEmit
pnpm -C ts exec eslint packages/paigasus-console-core
```

Expected: all PASS, including `tests/unit/discovery-redis-client.test.ts` and the existing
`tests/integration/discovery.test.ts` case "with an unreachable Redis … logs the failure once"
(it still sees exactly one `discovery.redis_connect_failed`, now always `connect_timeout`). `tsc`
and `eslint` exit 0. If `tsc` rejects `watchConnectionLoss(client, log)` in
`redisDescriptorCache`, report the exact error; do not cast.

- [ ] **Step 6: Run the container tier and see T1–T5 pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-console-core-ts:test-e2e
moon run paigasus-console-core-ts:test-e2e
```

Expected: both runs PASS, all five tests (the task is `cache: false`, so both runs execute). Run
twice to catch a timing flake. If T4 fails with `reason: 'other'` instead of `socket_closed`, do
not loosen the assertion; report it with the output (a TCP RST instead of a FIN is the likely
cause and needs a decision).

- [ ] **Step 7: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write packages/paigasus-console-core/src/discovery.ts packages/paigasus-console-core/src/logger.ts packages/paigasus-console-core/tests/unit/discovery-redis-client.test.ts
git add ts/packages/paigasus-console-core/src/discovery.ts ts/packages/paigasus-console-core/src/logger.ts ts/packages/paigasus-console-core/tests/unit/discovery-redis-client.test.ts
git commit -F - <<'EOF'
fix(ts): keep the descriptor cache Redis client alive after an idle gap (SMA-648)

socketTimeout is an idle timer, and the default reconnect strategy refuses a
socket timeout, so the first idle gap closed the client for good. The client
now pings every timeout and reconnects for every cause. A real loss logs one
discovery.redis_connection_lost line with a fixed reason and no DSN. The
connect-rejection branch no longer logs. T1 to T5 now pass.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

## Task 4: D4 preconditions and D5 doc corrections in @paigasus/discovery

**Files:**
- Modify: `ts/packages/paigasus-discovery/src/adapters/redis-cache.ts` (lines 1-2, 49-66, insert after line 92)
- Rewrite: `ts/packages/paigasus-discovery/tests/redis-cache-options.test.ts`
- Modify: `ts/packages/paigasus-discovery/tests/containers/support/redis.ts:15-25`
- Modify: `ts/packages/paigasus-discovery/README.md:87-100`

**Interfaces:**
- Consumes: `SocketTimeoutError` from `'redis'` (`redis` is already a runtime dependency, `package.json:23`; `new SocketTimeoutError(timeout: number)` is declared in `@redis/client/dist/lib/errors.d.ts:10-12` and re-exported by `redis` through `export * from '@redis/client'`).
- Produces: `createRedisDescriptorCache(client, options?)` (same signature) with six preconditions. Unique message fragments, one per precondition:
  - `created with \`disableOfflineQueue: true\``
  - `must have an error listener`
  - `created with \`commandOptions: { timeout }\``
  - `created with \`socket: { socketTimeout }\``
  - `created with \`pingInterval\``
  - `created with a \`socket.reconnectStrategy\``

- [ ] **Step 1: Write the failing test**

Replace `ts/packages/paigasus-discovery/tests/redis-cache-options.test.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
import { SocketTimeoutError } from 'redis';
import { describe, expect, it } from 'vitest';
import { createRedisDescriptorCache } from '../src/adapters/redis-cache.js';

// A client that satisfies ALL SIX preconditions. Each row below breaks exactly one of them.
const VALID_OPTIONS = {
  disableOfflineQueue: true,
  commandOptions: { timeout: 1_000 },
  socket: { socketTimeout: 2_000, reconnectStrategy: (): number => 100 },
  pingInterval: 1_000,
};

// One fragment per precondition, and each fragment occurs ONLY in that precondition's message
// (SMA-648 § 4.3). `expectRefusedBy` also asserts that the message matches no OTHER fragment, so a
// deleted check cannot hide behind a later check's message.
const DISABLE_OFFLINE_QUEUE = /created with `disableOfflineQueue: true`/;
const ERROR_LISTENER = /must have an error listener/;
const COMMAND_TIMEOUT = /created with `commandOptions: \{ timeout \}`/;
const SOCKET_TIMEOUT = /created with `socket: \{ socketTimeout \}`/;
const PING_INTERVAL = /created with `pingInterval`/;
const RECONNECT_STRATEGY = /created with a `socket\.reconnectStrategy`/;
const FRAGMENTS = [DISABLE_OFFLINE_QUEUE, ERROR_LISTENER, COMMAND_TIMEOUT, SOCKET_TIMEOUT, PING_INTERVAL, RECONNECT_STRATEGY];

function fakeClient(over: Record<string, unknown> = {}): never {
  return { options: VALID_OPTIONS, listenerCount: () => 1, ...over } as never;
}

function withOptions(over: Record<string, unknown>): never {
  return fakeClient({ options: { ...VALID_OPTIONS, ...over } });
}

function withSocket(over: Record<string, unknown>): never {
  return withOptions({ socket: { ...VALID_OPTIONS.socket, ...over } });
}

function expectRefusedBy(client: never, fragment: RegExp): void {
  let message: string | undefined;
  try {
    createRedisDescriptorCache(client);
  } catch (error) {
    message = error instanceof Error ? error.message : 'a non-Error was thrown';
  }
  expect(message, 'createRedisDescriptorCache accepted the client').toBeDefined();
  expect(message).toMatch(fragment);
  for (const other of FRAGMENTS.filter((candidate) => candidate !== fragment)) {
    expect(message).not.toMatch(other);
  }
}

describe('createRedisDescriptorCache preconditions', () => {
  it('accepts a correctly configured client', () => {
    expect(() => createRedisDescriptorCache(fakeClient())).not.toThrow();
  });

  it('refuses a client without disableOfflineQueue', () => {
    // Without it a Redis outage becomes HUNG REQUESTS rather than fast failures — node-redis
    // queues commands while disconnected. @paigasus/auth records this as load-bearing.
    expectRefusedBy(withOptions({ disableOfflineQueue: false }), DISABLE_OFFLINE_QUEUE);
  });

  it('refuses a client with no error listener', () => {
    // node-redis emits 'error' on an EventEmitter; with no listener Node crashes the process.
    expectRefusedBy(fakeClient({ listenerCount: () => 0 }), ERROR_LISTENER);
  });

  it('refuses a client with no command timeout', () => {
    // Without a per-command timeout a hung Redis blocks the render path indefinitely — the same
    // class of failure disableOfflineQueue guards against, on a path that check cannot reach.
    expectRefusedBy(withOptions({ commandOptions: undefined }), COMMAND_TIMEOUT);
  });

  it('refuses a client with a non-positive command timeout', () => {
    expectRefusedBy(withOptions({ commandOptions: { timeout: 0 } }), COMMAND_TIMEOUT);
  });

  it('refuses a client missing socketTimeout', () => {
    // commandOptions.timeout only bounds a command while it is QUEUED. socketTimeout is the only
    // bound on a command in flight, and only while the socket is otherwise silent.
    expectRefusedBy(withSocket({ socketTimeout: undefined }), SOCKET_TIMEOUT);
  });

  it('refuses a client with a non-positive socketTimeout', () => {
    expectRefusedBy(withSocket({ socketTimeout: 0 }), SOCKET_TIMEOUT);
  });

  // SMA-648 D4 (a). socketTimeout is an IDLE timer: without a ping, a quiet, healthy connection is
  // closed. The ping must be at most half of socketTimeout, which leaves one ping interval of
  // margin for event-loop lag.
  it('refuses a client missing pingInterval', () => {
    expectRefusedBy(withOptions({ pingInterval: undefined }), PING_INTERVAL);
  });

  it('refuses a client with a pingInterval of 0', () => {
    expectRefusedBy(withOptions({ pingInterval: 0 }), PING_INTERVAL);
  });

  it.each([Number.NaN, Number.POSITIVE_INFINITY])('refuses a client with a pingInterval that is not finite (%s)', (pingInterval) => {
    expectRefusedBy(withOptions({ pingInterval }), PING_INTERVAL);
  });

  it('refuses a client with a pingInterval above socketTimeout / 2', () => {
    expectRefusedBy(withOptions({ pingInterval: 1_001 }), PING_INTERVAL);
  });

  it('accepts a pingInterval of exactly socketTimeout / 2', () => {
    expect(() => createRedisDescriptorCache(withOptions({ pingInterval: 1_000 }))).not.toThrow();
  });

  // SMA-648 D4 (b). node-redis's DEFAULT strategy (`undefined`) returns false for a
  // SocketTimeoutError, so the first idle-timer close, or one lost PING reply, would close the
  // client for the life of the process.
  it('refuses a client with no reconnectStrategy (node-redis default)', () => {
    expectRefusedBy(withSocket({ reconnectStrategy: undefined }), RECONNECT_STRATEGY);
  });

  it('refuses a client with reconnectStrategy false', () => {
    expectRefusedBy(withSocket({ reconnectStrategy: false }), RECONNECT_STRATEGY);
  });

  it('refuses a strategy function that returns false', () => {
    expectRefusedBy(withSocket({ reconnectStrategy: () => false }), RECONNECT_STRATEGY);
  });

  it('refuses a strategy function that returns an Error', () => {
    expectRefusedBy(withSocket({ reconnectStrategy: () => new Error('stop') }), RECONNECT_STRATEGY);
  });

  it('refuses a strategy that returns false only for a SocketTimeoutError (the default strategy shape)', () => {
    // Proves the probe passes a REAL SocketTimeoutError, not just any error.
    expectRefusedBy(withSocket({ reconnectStrategy: (_retries: number, cause: Error) => (cause instanceof SocketTimeoutError ? false : 100) }), RECONNECT_STRATEGY);
  });

  it('refuses a strategy function that throws', () => {
    expectRefusedBy(
      withSocket({
        reconnectStrategy: () => {
          throw new Error('boom');
        },
      }),
      RECONNECT_STRATEGY,
    );
  });

  it.each([Number.NaN, -1])('refuses a strategy function that returns %s', (delay) => {
    expectRefusedBy(withSocket({ reconnectStrategy: () => delay }), RECONNECT_STRATEGY);
  });

  it.each([0, 500])('accepts a numeric reconnectStrategy (%s)', (reconnectStrategy) => {
    expect(() => createRedisDescriptorCache(withSocket({ reconnectStrategy }))).not.toThrow();
  });

  it('calls a strategy function once, with retries 0 and a SocketTimeoutError', () => {
    const calls: unknown[][] = [];
    const reconnectStrategy = (...args: unknown[]): number => {
      calls.push(args);
      return 0;
    };
    expect(() => createRedisDescriptorCache(withSocket({ reconnectStrategy }))).not.toThrow();
    expect(calls).toHaveLength(1);
    expect(calls[0]?.[0]).toBe(0);
    expect(calls[0]?.[1]).toBeInstanceOf(SocketTimeoutError);
  });
});
```

- [ ] **Step 2: Run it and see it fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/redis-cache-options.test.ts
```

Expected: FAIL. Every `pingInterval` row and every "refuses … strategy" row fails with
`createRedisDescriptorCache accepted the client`. The `calls` row fails on `toHaveLength(1)`
(0 calls). The rows for the four existing preconditions PASS already. If one of them fails on a
"not.toMatch(other)" check, an existing message holds another precondition's fragment; fix that
message in Step 3.

- [ ] **Step 3: Implement D4 and the D5 comment**

In `ts/packages/paigasus-discovery/src/adapters/redis-cache.ts`:

(1) Replace line 2:

```ts
import type { RedisClientType } from 'redis';
```

with:

```ts
import { SocketTimeoutError, type RedisClientType } from 'redis';
```

(2) Replace the doc comment at lines 49-66 (from `/**` above `createRedisDescriptorCache` to the
line before `export function createRedisDescriptorCache`) with:

```ts
/**
 * Wrap an ALREADY-CONNECTED node-redis client.
 *
 * The package never opens a connection: the composition root decides whether to share
 * @paigasus/auth's client or open a second one, and that decision is out of this issue's scope.
 *
 * The six preconditions below are ASSERTED rather than documented. A precondition nobody checks
 * is a precondition nobody keeps, and all six failures are silent until production.
 *
 * The two timeouts bound DIFFERENT phases and neither substitutes for the other.
 * `commandOptions.timeout` bounds a command only while it is QUEUED, client-side, before it is
 * written to the socket. Once node-redis has written the command, `commandOptions.timeout` no
 * longer applies.
 *
 * `socket.socketTimeout` is an IDLE timer, not a reply deadline (SMA-648, read from @redis/client
 * 6.2.1 socket.js). node-redis destroys the socket with a SocketTimeoutError after that many
 * milliseconds with no socket activity, and any read OR write resets the timer. So it bounds a
 * command in flight only while the socket is otherwise silent: under steady traffic each new write
 * moves the deadline, and a Redis that accepts commands and never replies can go unnoticed
 * (follow-up SMA-650). It also fires on a quiet, healthy connection. Two more preconditions make
 * that safe. `pingInterval` keeps an idle socket alive; it must be at most half of `socketTimeout`,
 * which leaves one ping interval of margin for event-loop lag. And `socket.reconnectStrategy` must
 * return a delay for a SocketTimeoutError: node-redis's default strategy returns `false` for that
 * cause, so without one the first idle-timer close, or one lost PING reply, closes the client for
 * the life of the process.
 */
```

(3) Replace the existing `socketTimeout` refusal (lines 84-92) with the corrected message
(D5), and insert the two new checks directly after it, before `const prefix = …`:

```ts
  const socketTimeoutMs = client.options?.socket?.socketTimeout;
  if (typeof socketTimeoutMs !== 'number' || !Number.isFinite(socketTimeoutMs) || socketTimeoutMs <= 0) {
    throw new Error(
      'createRedisDescriptorCache: the client must be created with `socket: { socketTimeout }` set to a ' +
        'positive, finite number of milliseconds. It is an idle timer that any read or write resets, and it is ' +
        'the only bound on a command already written to the socket, while the socket is otherwise silent. ' +
        '`commandOptions.timeout` bounds a command only while it is queued',
    );
  }
  const pingIntervalMs = client.options?.pingInterval;
  if (typeof pingIntervalMs !== 'number' || !Number.isFinite(pingIntervalMs) || pingIntervalMs <= 0 || pingIntervalMs > socketTimeoutMs / 2) {
    throw new Error(
      'createRedisDescriptorCache: the client must be created with `pingInterval` set to a positive, finite ' +
        'number of milliseconds of at most half of socketTimeout. socketTimeout is an idle timer, so without a ' +
        'ping a quiet, healthy connection is closed; the half leaves one ping interval of margin for event-loop lag',
    );
  }
  if (!reconnectsAfterSocketTimeout(client.options?.socket?.reconnectStrategy, socketTimeoutMs)) {
    throw new Error(
      'createRedisDescriptorCache: the client must be created with a `socket.reconnectStrategy` that returns a ' +
        'finite, non-negative delay for a SocketTimeoutError. The node-redis default returns false for that cause, ' +
        'so the first idle-timer close, or one lost PING reply, would close the client for the life of the process',
    );
  }
```

Check the fragments by eye before you go on: the `socketTimeout` message says "`socketTimeout`"
only inside "`socket: { socketTimeout }`" and mentions no `` `pingInterval` ``; the ping message
says "socketTimeout" WITHOUT backticks; neither new message holds "created with `socket:",
"created with `commandOptions", "created with `disableOfflineQueue" or "must have an error
listener".

(4) Add this function above the `createRedisDescriptorCache` doc comment (after the
`RedisDescriptorCacheOptions` type):

```ts
/**
 * SMA-648 D4 (b): true when node-redis would reconnect after a socket timeout. `undefined` means
 * node-redis's default strategy, which returns `false` for a SocketTimeoutError, and `false` never
 * reconnects. A number always reconnects. A function is called ONCE, with the arguments node-redis
 * passes on a first timeout, and must return a finite, non-negative number.
 */
function reconnectsAfterSocketTimeout(strategy: unknown, socketTimeoutMs: number): boolean {
  if (typeof strategy === 'number') return true;
  if (typeof strategy !== 'function') return false;
  let delay: unknown;
  try {
    delay = (strategy as (retries: number, cause: Error) => unknown)(0, new SocketTimeoutError(socketTimeoutMs));
  } catch {
    return false;
  }
  return typeof delay === 'number' && Number.isFinite(delay) && delay >= 0;
}
```

- [ ] **Step 4: Give the container-tier support client the two new options**

In `ts/packages/paigasus-discovery/tests/containers/support/redis.ts`, replace lines 15-25:

```ts
/**
 * A client carrying the options createRedisDescriptorCache asserts. Mirrors the production shape
 * in @paigasus/auth's createRedisSessionStore.
 */
export async function connect(url: string): Promise<RedisClientType> {
  const client: RedisClientType = createClient({ url, disableOfflineQueue: true, commandOptions: { timeout: 5_000 }, socket: { socketTimeout: 10_000 } });
  // Never log the raw error: node-redis embeds the DSN in its connection errors.
  client.on('error', () => undefined);
  await client.connect();
  return client;
}
```

with:

```ts
/**
 * A client carrying the six options createRedisDescriptorCache asserts. `pingInterval` is half of
 * `socketTimeout`, the largest value the precondition allows, and the reconnect strategy returns a
 * delay for every cause, a socket timeout included (SMA-648 D4).
 */
export async function connect(url: string): Promise<RedisClientType> {
  const client: RedisClientType = createClient({
    url,
    disableOfflineQueue: true,
    commandOptions: { timeout: 5_000 },
    pingInterval: 5_000,
    socket: { socketTimeout: 10_000, reconnectStrategy: (retries: number) => Math.min(retries * 100, 2_000) },
  });
  // Never log the raw error: node-redis embeds the DSN in its connection errors.
  client.on('error', () => undefined);
  await client.connect();
  return client;
}
```

- [ ] **Step 5: Correct the README (D5)**

In `ts/packages/paigasus-discovery/README.md`, replace lines 87-100 (from
"The Redis client is **injected and already connected**." to "Both must be positive, finite
numbers of milliseconds.") with:

```markdown
The Redis client is **injected and already connected**. It must be created with
`disableOfflineQueue: true`, an `error` listener, `commandOptions: { timeout }`,
`socket: { socketTimeout }`, `pingInterval`, and a `socket.reconnectStrategy`
that accepts a socket timeout — all six are asserted. Without the first a Redis
outage becomes hung page renders. Without the second node-redis crashes the
process. Without the third and fourth a hung Redis blocks the render path
instead of failing fast. Without the fifth a quiet, healthy connection is
closed. Without the sixth that close is permanent. The error listener must
never log the raw error, which embeds the DSN.

The two timeouts bound different phases and neither substitutes for the other.
`commandOptions.timeout` bounds a command only while it is QUEUED, before it is
written to the socket. `socket.socketTimeout` is an IDLE timer, not a reply
deadline: node-redis destroys the socket when nothing is read or written for
that many milliseconds, and any read OR write resets it. So it bounds a command
in flight only while the socket is otherwise silent. Under steady traffic each
new write moves the deadline, and a Redis that accepts commands and never
replies can go unnoticed (SMA-650). Both must be positive, finite numbers of
milliseconds.

Because `socketTimeout` also fires on a quiet, healthy connection, two more
options are required. `pingInterval` keeps an idle socket alive: it must be a
positive number of at most half of `socketTimeout`. The reconnect strategy must
return a delay for a `SocketTimeoutError`: node-redis's default strategy returns
`false` for that cause, so without one the first idle gap closes the client for
the life of the process. The Redis user needs the `+ping` ACL permission.
```

Then search for any other copy of the old claim and fix it the same way:

```bash
grep -rn -e "end-to-end deadline" -e "all four are asserted" -e "four preconditions" -e "no reply arrives within" ts/packages --include='*.ts' --include='*.md' | grep -v node_modules
```

Expected: no output. If a line remains, rewrite it in the wording above.

- [ ] **Step 6: Run the tests and see them pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-discovery exec vitest run
pnpm -C ts/packages/paigasus-discovery exec tsc -p tsconfig.json --noEmit
pnpm -C ts/packages/paigasus-console-core exec vitest run
moon run paigasus-discovery-ts:test-e2e paigasus-console-core-ts:test-e2e
pnpm -C ts exec eslint packages/paigasus-discovery packages/paigasus-console-core
```

Expected: all PASS. `paigasus-discovery-ts:test-e2e` proves the updated `connect()` satisfies D4
(the redis contract suite and the multiprocess single-flight test both build a cache from it).
`paigasus-console-core-ts:test-e2e` and the console-core unit tests prove the console-core client
satisfies D4 (the options pin and the unreachable-Redis integration case both go through
`createRedisDescriptorCache`). If `paigasus-discovery-ts:test-e2e` fails in Playwright for a
missing browser, run `pnpm -C ts/packages/paigasus-discovery exec playwright install chromium`
and re-run.

- [ ] **Step 7: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write packages/paigasus-discovery/src/adapters/redis-cache.ts packages/paigasus-discovery/tests/redis-cache-options.test.ts packages/paigasus-discovery/tests/containers/support/redis.ts packages/paigasus-discovery/README.md
git add ts/packages/paigasus-discovery/src/adapters/redis-cache.ts ts/packages/paigasus-discovery/tests/redis-cache-options.test.ts ts/packages/paigasus-discovery/tests/containers/support/redis.ts ts/packages/paigasus-discovery/README.md
git commit -F - <<'EOF'
fix(ts): refuse a descriptor cache client that closes after an idle gap (SMA-648)

createRedisDescriptorCache now also asserts a pingInterval of at most half of
socketTimeout, and a reconnect strategy that returns a delay for a
SocketTimeoutError. Each precondition has a message fragment of its own. The
socketTimeout docs said it was a reply deadline; it is an idle timer.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

## Task 5: CI registry fallout in ci/affected-graph/run.sh

**Files:**
- Modify: `ci/affected-graph/run.sh:89-99` (the "FIVE" comment)
- Modify: `ci/affected-graph/run.sh:494-528` (the two discovery cases), `:657-676` (the two console-core cases) — ONLY where the measurement in Step 1 shows a change.

**Interfaces:**
- Consumes: the new `paigasus-console-core-ts:test-e2e` inputs (Task 2).
- Produces: re-baselined strict-equality expected sets, each DERIVED from `moon query tasks --affected`, never typed by hand.

- [ ] **Step 1: Measure every case anchored under the new task's inputs**

Write this script to your scratchpad as `sma648-measure.sh` and run it with `/bin/bash`
(the filter is the one `_assert_task_case_impl` uses, `ci/affected-graph/run.sh:106-114`):

```bash
#!/bin/bash
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-648
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
measure() {
  printf '%s\n' "$1" | moon query tasks --affected | python3 -c '
import sys, json
d = json.load(sys.stdin)
print(",".join(sorted(f"{p}:{n}" for p, ts in (d.get("tasks") or {}).items() for n in ts if n in ("build", "test", "lint", "test-e2e"))))'
}
for f in \
  ts/packages/paigasus-discovery/src/core/state.ts \
  ts/packages/paigasus-discovery/src/adapters/memory-cache.ts \
  ts/packages/paigasus-console-core/src/runtime.ts \
  ts/packages/paigasus-console-core/src/prn-tenancy.ts \
  ts/packages/paigasus-console-core/testing/fake-iam.ts; do
  echo "== $f"
  measure "$f"
done
echo "== projects that declare test-e2e"
moon query tasks | python3 -c 'import sys, json; d = json.load(sys.stdin); print(sorted(p for p, t in d["tasks"].items() if "test-e2e" in t))'
```

```bash
/bin/bash "<your scratchpad>/sma648-measure.sh"
```

Expected (verify, do not assume): the first four anchors each print their CURRENT expected set
plus `paigasus-console-core-ts:test-e2e`. The `testing/fake-iam.ts` anchor prints its current set
UNCHANGED (the new task does not list `testing/**/*`). The last line lists six projects:
`gateway-console-ts`, `iam-console-ts`, `paigasus-app-shell-ts`, `paigasus-auth-ts`,
`paigasus-console-core-ts`, `paigasus-discovery-ts`. If the output differs from this, re-baseline
what the measurement shows and report the difference.

- [ ] **Step 2: Re-baseline each changed case**

For each of the four cases whose measured set changed, replace the case's quoted expected-CSV
string with the MEASURED line from Step 1, pasted exactly (it is sorted; the order does not
matter to the harness). Add the comment line directly above each `run_task_case_ci` line, after
its existing SMA-512 PR 3 comment:

For `discovery->discovery-tasks` (line 509) and `discovery-adapters->discovery-tasks` (line 527):

```bash
  # SMA-648: paigasus-console-core-ts:test-e2e joins this set — the new Docker tier's inputs name
  # '/ts/packages/paigasus-discovery/src/**/*', because that package owns the preconditions the
  # tier's Redis client must meet. MEASURED with the no-flag `moon query tasks --affected`.
```

For the `console-core->consumers` / `console-core-prn-tenancy->consumers` pair, add this ONCE,
directly above line 673:

```bash
  # SMA-648: paigasus-console-core-ts:test-e2e joins BOTH sets — the new Docker tier's inputs name
  # this package's own `src/**/*`. It is correctly ABSENT from the console-core-testing case below:
  # the tier imports nothing from testing/, so its inputs do not list it. MEASURED with the no-flag
  # `moon query tasks --affected`.
```

For reference, the measured sets are expected to be (take the real values from Step 1):

```
discovery->discovery-tasks and discovery-adapters->discovery-tasks:
gateway-console-ts:build,gateway-console-ts:test,gateway-console-ts:test-e2e,iam-console-ts:build,iam-console-ts:test,iam-console-ts:test-e2e,paigasus-app-shell-ts:build,paigasus-app-shell-ts:test,paigasus-app-shell-ts:test-e2e,paigasus-console-core-ts:build,paigasus-console-core-ts:test,paigasus-console-core-ts:test-e2e,paigasus-discovery-ts:build,paigasus-discovery-ts:test,paigasus-discovery-ts:test-e2e,ts:lint

console-core->consumers and console-core-prn-tenancy->consumers:
gateway-console-ts:build,gateway-console-ts:test,gateway-console-ts:test-e2e,iam-console-ts:build,iam-console-ts:test,iam-console-ts:test-e2e,paigasus-console-core-ts:build,paigasus-console-core-ts:test,paigasus-console-core-ts:test-e2e,ts:lint
```

- [ ] **Step 3: Update the "FIVE" comment**

In `ci/affected-graph/run.sh`, replace lines 89-91:

```bash
#   Measured (SMA-510, SMA-511, SMA-512): FIVE projects now declare `test-e2e` —
#   `paigasus-auth-ts`, `paigasus-discovery-ts`, `paigasus-app-shell-ts`, `iam-console-ts` and
#   `gateway-console-ts`. app-shell's `test-e2e`
```

with:

```bash
#   Measured (SMA-510, SMA-511, SMA-512, SMA-648): SIX projects now declare `test-e2e` —
#   `paigasus-auth-ts`, `paigasus-discovery-ts`, `paigasus-app-shell-ts`, `iam-console-ts`,
#   `gateway-console-ts` and `paigasus-console-core-ts`. console-core's `test-e2e` (SMA-648) keys
#   on its own `src/**/*` and `tests/**/*` and on `@paigasus/discovery`'s `src/**/*`, so the
#   discovery and console-core source cases below include it; it does NOT key on `testing/**/*`,
#   so the console-core-testing case does not. app-shell's `test-e2e`
```

Lines 92-99 stay as they are.

- [ ] **Step 4: Run the gate through Moon, with system bash**

`ci/affected-graph/run.sh` deadlocks under Homebrew bash 5.3.15 on this machine, and `/bin` first
on PATH downgrades `python3` to 3.9 (no `tomllib`). Shim ONLY bash:

```bash
mkdir -p "${TMPDIR:-/tmp}/sma648-bashshim"
ln -sf /bin/bash "${TMPDIR:-/tmp}/sma648-bashshim/bash"
PATH="${TMPDIR:-/tmp}/sma648-bashshim:$HOME/.proto/shims:$HOME/.proto/bin:$PATH" moon run repo:affected-smoke --force
```

Expected: exit 0. The negative control prints its own designed FAIL rows, then the real suite
prints `PASS` for `discovery->discovery-tasks`, `discovery-adapters->discovery-tasks`,
`console-core->consumers`, `console-core-prn-tenancy->consumers` and
`console-core-testing->consumers`. If any OTHER case fails, measure it with the Step 1 `measure`
function, and re-baseline it only if the change follows from this issue's inputs; otherwise stop
and report.

- [ ] **Step 5: Commit**

```bash
git add ci/affected-graph/run.sh
git commit -F - <<'EOF'
test(ci): re-baseline the affected-graph cases for the console-core e2e tier (SMA-648)

paigasus-console-core-ts:test-e2e keys on console-core's and discovery's
sources, so four strict-equality cases gain it. The sets are measured with
moon query tasks --affected, not typed. The hand count of projects that
declare test-e2e becomes six.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

## Task 6: CLAUDE.md Gotchas entry (D5 wording)

**Files:**
- Modify: `CLAUDE.md` — insert one bullet directly before the line that starts `- **\`forbidden()\` needs \`experimental.authInterrupts\`` (line 1055 today).

**Interfaces:**
- Consumes: nothing. Produces: documentation only. Do NOT touch the `ci-targets` or `moon-diagnosis` markers, and do not quote them.

- [ ] **Step 1: Insert the entry**

Insert, directly before `- **\`forbidden()\` needs \`experimental.authInterrupts\`, and a React \`cache()\` value does not reach`:

```markdown
- **node-redis `socket.socketTimeout` is an IDLE timer, not a reply deadline** (read from
  `@redis/client` 6.2.1, SMA-648). Any read OR write on the socket resets it, so it fires on a
  quiet, healthy connection, and it bounds a hung command only while the socket is otherwise
  silent — under steady traffic a Redis that accepts commands and never replies is still unbounded
  (follow-up SMA-650). node-redis's DEFAULT reconnect strategy returns `false` for a
  `SocketTimeoutError`, so the first idle gap closed the console descriptor cache's client for the
  life of the process (`The client is closed`, nav degraded). A client that sets `socketTimeout`
  therefore needs `pingInterval` (at most half of `socketTimeout`) to keep an idle socket alive, and
  a `socket.reconnectStrategy` that returns a delay for EVERY cause. `createRedisDescriptorCache`
  asserts all six options; the Redis user needs `+ping`. `paigasus-console-core-ts:test-e2e` is the
  Docker-backed control, with no skip hatch, so a console-core source edit now needs Docker.
```

- [ ] **Step 2: Check the markers are untouched**

```bash
grep -c "ci-targets:begin" CLAUDE.md
grep -c "ci-targets:end" CLAUDE.md
grep -c "moon-diagnosis:begin" CLAUDE.md
git diff --stat CLAUDE.md
```

Expected: `1`, `1`, `1`, and a `CLAUDE.md` diff stat with insertions only (no deletions).

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -F - <<'EOF'
docs(ts): record that node-redis socketTimeout is an idle timer (SMA-648)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

## Task 7: Final verification

**Files:** none changed (unless a check fails; then fix in a NEW commit).

**Interfaces:** none.

- [ ] **Step 1: Confirm the task names**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query tasks | python3 -c '
import sys, json
d = json.load(sys.stdin)["tasks"]
for p in ("paigasus-console-core-ts", "paigasus-discovery-ts", "ts"):
    print(p, sorted(d[p]))
print("repo:affected-smoke" if "affected-smoke" in d["repo"] else "MISSING repo:affected-smoke")'
```

Expected: `paigasus-console-core-ts ['build', 'test', 'test-e2e', 'typecheck']`,
`paigasus-discovery-ts ['build', 'test', 'test-e2e', 'typecheck']`,
`ts ['check-config-only', 'commitlint', 'fmt', 'lint']`, `repo:affected-smoke`.

- [ ] **Step 2: Run the package, lint and format tasks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-console-core-ts:test paigasus-console-core-ts:test-e2e paigasus-console-core-ts:typecheck paigasus-discovery-ts:test paigasus-discovery-ts:test-e2e paigasus-discovery-ts:typecheck ts:lint ts:fmt iam-console-ts:test gateway-console-ts:test --force
```

Expected: every task PASSES. (`iam-console-ts:test` and `gateway-console-ts:test` consume
`@paigasus/console-core`; they are cheap and need no Docker.)

- [ ] **Step 3: Prettier check on every changed file**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git diff --name-only origin/main...HEAD -- ts/ | sed 's|^ts/||' | xargs pnpm -C ts exec prettier --check
```

Expected: `All matched files use Prettier code style!` (`pnpm-lock.yaml` is in
`.prettierignore`; Prettier reports it as ignored, which is fine).

- [ ] **Step 4: Run the affected-graph gate through Moon**

```bash
mkdir -p "${TMPDIR:-/tmp}/sma648-bashshim"
ln -sf /bin/bash "${TMPDIR:-/tmp}/sma648-bashshim/bash"
PATH="${TMPDIR:-/tmp}/sma648-bashshim:$HOME/.proto/shims:$HOME/.proto/bin:$PATH" moon run repo:affected-smoke --force
```

Expected: exit 0.

- [ ] **Step 5: Check the commit list and the tree**

```bash
git status --short
git log --oneline origin/main..HEAD
```

Expected: a clean tree. On top of the spec commits (and the plan commit, if the controller made
one) there are exactly six implementation commits: the D10 fix, the red tier, the console-core
fix, the discovery preconditions, the affected-graph re-baseline and the CLAUDE.md entry.

---

## Manual step for the controller (not a subagent task)

The agent memory is outside the repo. After Task 7, the controller corrects it (spec § 4.6):

- `/Users/smaschek/.claude/projects/-Users-smaschek-dev-paigasus-paigasus-core/memory/node-redis-command-vs-socket-timeout.md`: replace the claim that `socketTimeout` is "the end-to-end deadline" with the D5 wording (idle timer; any read OR write resets it; bounds a hung command only while the socket is otherwise silent; `pingInterval` keeps an idle socket alive; a reconnect strategy must accept a `SocketTimeoutError`; SMA-648, SMA-650).
- The matching index line in `MEMORY.md` ("node-redis command vs socket timeout"): replace "`socket.socketTimeout` is the end-to-end deadline" with "`socket.socketTimeout` is an IDLE timer that reads and writes reset, and the default reconnect strategy refuses its error (SMA-648)".
