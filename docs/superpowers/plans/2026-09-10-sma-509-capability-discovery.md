# `@paigasus/discovery` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `@paigasus/discovery`, a server-only package that resolves each configured Paigasus service into one of three states — absent, available, degraded — from its `ServiceInfo` descriptor, cached in Redis with stale-while-revalidate and single-flight.

**Architecture:** A probe issues `GET /v1/service-info` over plain `fetch` against an operator-configured base URL, carrying the request's bearer token. The result lands in a `DescriptorCache` port (Redis and in-memory adapters) as a record that tracks the last good descriptor and the last probe outcome **independently**, so a failed probe reports `degraded` while still serving the known feature set. A `<Capability need="…">` async server component renders children normally, hidden, or disabled-with-a-reason.

**Tech Stack:** TypeScript 6, React 19, node-redis v6, zod 4, vitest 5, testcontainers, Moon.

**Spec:** `docs/superpowers/specs/2026-09-10-sma-509-capability-discovery-design.md`

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- The package is `private: true`, source-only (`exports` point at `.ts`), no build step.
- **No `@paigasus/sdk` dependency.** Spec F8 and §2.1.
- **Never set the `react-server` vitest condition.** Conditions are `['node', 'import', 'default']`; `server-only` is handled by a `resolve.alias` to an empty stub. Spec §11.
- Moon tasks **append** to `.moon/tasks/typescript-project.yml`; never `options.merge: replace`.
- All shell commands run from the worktree root `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-509` unless stated otherwise, with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Timing orderings asserted at construction: `NEGATIVE_MS < FRESH_MS < STALE_MS` and `PROBE_TIMEOUT_MS < LOCK_WAIT_MS < LOCK_TTL_MS`.
- `401`/`403` are caller-scoped and **never written to the cache**. Spec §7.2.
- The cache key's `<service>` is always the config key, never `descriptor.service`. Spec F7.
- Run `moon run ts:fmt` after the last TypeScript edit; it is a separate whole-tree Prettier gate.

---

### Task 1: Package scaffold, types, and the capability vocabulary

Creates the package so every later task has somewhere to land, and derives the two closed vocabularies (capability keys, service slugs) from the proto registry.

**Files:**
- Create: `ts/packages/paigasus-discovery/package.json`
- Create: `ts/packages/paigasus-discovery/tsconfig.json`
- Create: `ts/packages/paigasus-discovery/vitest.config.ts`
- Create: `ts/packages/paigasus-discovery/tests/support/server-only-stub.ts`
- Create: `ts/packages/paigasus-discovery/src/types.ts`
- Create: `ts/packages/paigasus-discovery/src/core/state.ts`
- Test: `ts/packages/paigasus-discovery/tests/vocabulary.test.ts`

**Interfaces:**
- Consumes: `capabilityWireKey`, `Capability`, `CapabilitySchema` from `@paigasus/proto`.
- Produces: `CAPABILITY_KEYS: readonly string[]`, `SERVICE_SLUGS: readonly string[]`, `type CapabilityKey`, `type ServiceDescriptor`, `type DegradedReason`, `type ServiceState`, `SERVICE_STATES`.

- [ ] **Step 1: Create the package manifest**

`ts/packages/paigasus-discovery/package.json`:

```json
{
  "name": "@paigasus/discovery",
  "_comment_exports": "THREE subpath exports and deliberately NO \".\". A root export re-exporting the server surface would let a client component import it and walk around the boundary — the same reason @paigasus/auth omits one. tests/structure/exports.test.ts pins this key set.",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "engines": {
    "node": ">=24"
  },
  "license": "Apache-2.0",
  "description": "Lazy, request-scoped service capability discovery with three service states for Paigasus console zones.",
  "exports": {
    "./server": "./src/server.ts",
    "./react": "./src/react.tsx",
    "./types": "./src/types.ts"
  },
  "scripts": {
    "typecheck": "tsc -p tsconfig.json --noEmit"
  },
  "dependencies": {
    "@paigasus/proto": "workspace:*",
    "redis": "catalog:",
    "server-only": "catalog:",
    "zod": "catalog:"
  },
  "peerDependencies": {
    "react": "catalog:"
  },
  "devDependencies": {
    "@testing-library/dom": "catalog:",
    "@testing-library/jest-dom": "catalog:",
    "@testing-library/react": "catalog:",
    "@testing-library/user-event": "catalog:",
    "@types/node": "catalog:",
    "@types/react": "catalog:",
    "@types/react-dom": "catalog:",
    "jsdom": "catalog:",
    "react": "catalog:",
    "react-dom": "catalog:",
    "testcontainers": "catalog:",
    "typescript": "catalog:",
    "vitest": "catalog:"
  }
}
```

- [ ] **Step 2: Create the tsconfig**

`ts/packages/paigasus-discovery/tsconfig.json`:

```json
{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "lib": ["DOM", "DOM.Iterable", "ES2022"],
    "jsx": "react-jsx",
    "types": ["node"],
    "noEmit": true
  },
  "_comment_include": "tests/ and both vitest configs must be in the program: ts/eslint.config.js applies type-checked rules with projectService:true to every **/*.{ts,tsx}, and `ts:lint` runs `eslint .` over the whole tree — a file in no program errors there. Same shape and same reason as ts/packages/paigasus-auth/tsconfig.json.",
  "include": ["src/**/*", "tests/**/*", "vitest.config.ts", "vitest.containers.config.ts"]
}
```

- [ ] **Step 3: Create the `server-only` stub and the vitest config**

`ts/packages/paigasus-discovery/tests/support/server-only-stub.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// A permanent test-only stand-in for the real `server-only` package (see vitest.config.ts's
// `resolve.alias`). The real package's default export is an unconditional throw. Aliasing it
// here — rather than adding the `react-server` resolution condition — is deliberate and is
// MEASURED in ts/packages/paigasus-auth/vitest.config.ts: `react-server` is also the condition
// `react`'s own exports map switches on, and that build has no `createContext`, so setting it
// breaks `react-dom/client` under @testing-library/react. There is no flat condition list that
// satisfies both. The alias applies to every test with nothing to remember per file.
export {};
```

`ts/packages/paigasus-discovery/vitest.config.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// NO `react-server` CONDITION. See tests/support/server-only-stub.ts for the measured reason.
// The list stays ADDITIVE: dropping `import`/`default` breaks source-exports `.ts` resolution
// for every @paigasus/* package.
const conditions = ['node', 'import', 'default'];

const serverOnlyStub = fileURLToPath(new URL('./tests/support/server-only-stub.ts', import.meta.url));

export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts', 'tests/**/*.test.tsx'],
    // The container-backed suites live under tests/containers/ and run only in the test-e2e task.
    exclude: ['tests/containers/**', '**/node_modules/**'],
    setupFiles: ['./tests/setup.ts'],
    // React Testing Library registers its automatic cleanup only when a global afterEach exists,
    // and vitest defaults globals to false. Without this the DOM accumulates between tests and
    // produces duplicate-id failures belonging to an EARLIER test (the trap @paigasus/ui records).
    globals: true,
  },
  resolve: { conditions, alias: { 'server-only': serverOnlyStub } },
  ssr: { resolve: { conditions } },
});
```

`ts/packages/paigasus-discovery/tests/setup.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import '@testing-library/jest-dom/vitest';
```

- [ ] **Step 4: Write the failing vocabulary test**

`ts/packages/paigasus-discovery/tests/vocabulary.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { Capability, CapabilitySchema, capabilityWireKey } from '@paigasus/proto';
import { CAPABILITY_KEYS, SERVICE_SLUGS, SERVICE_STATES } from '../src/core/state.js';

describe('capability vocabulary', () => {
  it('derives every non-sentinel capability key from the proto registry', () => {
    // Derived here INDEPENDENTLY of the implementation, from the generated schema, so this test
    // fails if the implementation hand-tabulates a list that drifts from the proto.
    const expected = Object.values(Capability)
      .filter((v): v is Capability => typeof v === 'number')
      .map((v) => capabilityWireKey(v))
      .filter((k): k is string => k !== undefined)
      .sort();

    expect([...CAPABILITY_KEYS].sort()).toEqual(expected);
  });

  it('excludes the zero sentinel', () => {
    expect(CAPABILITY_KEYS).not.toContain(undefined);
    expect(capabilityWireKey(Capability.UNSPECIFIED)).toBeUndefined();
    expect(CAPABILITY_KEYS.length).toBe(Object.keys(CapabilitySchema.value).length - 1);
  });

  it('derives service slugs as the first dot-segment of every key', () => {
    expect([...SERVICE_SLUGS].sort()).toEqual(['gateway', 'iam']);
  });

  it('every capability key starts with one of the service slugs', () => {
    for (const key of CAPABILITY_KEYS) {
      expect(SERVICE_SLUGS).toContain(key.split('.')[0]);
    }
  });

  it('exposes the three state names', () => {
    expect(SERVICE_STATES).toEqual(['absent', 'available', 'degraded']);
  });
});
```

- [ ] **Step 5: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts install
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/vocabulary.test.ts
```

Expected: FAIL — `Failed to resolve import "../src/core/state.js"`.

- [ ] **Step 6: Write `src/types.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The client-safe entry. NO `server-only` guard, deliberately — a client component holds a
// resolved ServiceState as data, exactly as @paigasus/sdk's ./errors/types is exempted.
//
// Everything here must survive the RSC server-to-client boundary as plain data. That is why
// `capabilities` is a readonly array and not a ReadonlySet (a Set does not serialize), and why
// ServiceDescriptor is a plain structural type rather than protobuf-es's ServiceInfo message
// (which carries $typeName, and which apps are banned from naming — the
// `paigasus/boundaries/apps` eslint block bans @paigasus/proto including type imports).

/** A service's self-description, converted from `paigasus.common.v1.ServiceInfo`. */
export type ServiceDescriptor = {
  readonly service: string;
  readonly version: string;
  readonly capabilities: readonly string[];
};

/**
 * Why a configured service is not usable right now.
 *
 * A closed union, never free text: the UI branches on the code and never on a message.
 * This vocabulary is owned HERE and not imported from @paigasus/sdk — that package's
 * `Presentation` union cannot express these cases (spec F8).
 */
export type DegradedReason =
  | 'timeout'
  | 'network'
  | 'unauthorized'
  | 'not-implemented'
  | 'bad-response'
  | 'server-error'
  | 'cache-unavailable';

export type ServiceState =
  | { readonly state: 'absent'; readonly service: string }
  | {
      readonly state: 'available';
      readonly service: string;
      readonly descriptor: ServiceDescriptor;
      readonly capabilities: readonly string[];
    }
  | {
      readonly state: 'degraded';
      readonly service: string;
      readonly reason: DegradedReason;
      readonly descriptor: ServiceDescriptor | null;
      readonly capabilities: readonly string[];
    };

export type { CapabilityKey } from './core/state.js';
export { SERVICE_STATES } from './core/state.js';
```

- [ ] **Step 7: Write `src/core/state.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
import { Capability, CapabilitySchema, capabilityWireKey } from '@paigasus/proto';

/**
 * Every advertised capability key, derived from the generated registry rather than tabulated.
 *
 * A second copy of the registry would drift against the proto. `capabilityWireKey` reads the
 * descriptor's own value names, so this list moves the day the proto does.
 */
export const CAPABILITY_KEYS: readonly string[] = Object.keys(CapabilitySchema.value)
  .map((n) => capabilityWireKey(Number(n) as Capability))
  .filter((k): k is string => k !== undefined);

/**
 * The closed set of service slugs, derived as the first dot-segment of every capability key.
 *
 * The proto guarantees this shape: `ServiceInfo.service` is "a bare slug matching the prefix of
 * its own capability keys". This is what PAIGASUS_SERVICES keys are validated against, so an
 * operator typo fails construction instead of silently emptying the console.
 */
export const SERVICE_SLUGS: readonly string[] = [
  ...new Set(CAPABILITY_KEYS.map((k) => k.slice(0, k.indexOf('.')))),
];

export const SERVICE_STATES = ['absent', 'available', 'degraded'] as const;

/**
 * A capability key as a compile-time closed union.
 *
 * Typed as a template literal rather than `string` so `need="iam.audits"` is a type error. At
 * runtime a typo returns false forever and silently hides a nav item, which is the "invisible"
 * failure the design calls worse than a wrongly-shown disabled one.
 */
export type CapabilityKey = `${string}.${string}`;

/** Whether a key is in the registry this build knows about. */
export function isKnownCapability(key: string): boolean {
  return CAPABILITY_KEYS.includes(key);
}

/** The service slug a capability key belongs to. */
export function serviceOf(key: string): string {
  const dot = key.indexOf('.');
  return dot === -1 ? key : key.slice(0, dot);
}
```

- [ ] **Step 8: Run the test to verify it passes**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/vocabulary.test.ts
```

Expected: PASS, 5 tests.

- [ ] **Step 9: Commit**

```bash
git add ts/packages/paigasus-discovery
git commit -m "feat(ts): scaffold @paigasus/discovery and derive the capability vocabulary (SMA-509)"
```

---

### Task 2: The cache record, freshness, and the state mapping

The heart of the design: reachability and the capability list are independent fields, freshness reads `outcomeAt` in both arms, and `toState` is total.

**Files:**
- Create: `ts/packages/paigasus-discovery/src/core/record.ts`
- Test: `ts/packages/paigasus-discovery/tests/record.test.ts`

**Interfaces:**
- Consumes: `ServiceDescriptor`, `DegradedReason`, `ServiceState` from `src/types.ts`.
- Produces: `type CacheRecord`, `RECORD_VERSION`, `type Timings`, `DEFAULT_TIMINGS`, `isFresh(rec, now, t)`, `toState(service, rec)`, `parseRecord(raw)`.

- [ ] **Step 1: Write the failing test**

`ts/packages/paigasus-discovery/tests/record.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import {
  DEFAULT_TIMINGS,
  RECORD_VERSION,
  isFresh,
  parseRecord,
  toState,
  type CacheRecord,
} from '../src/core/record.js';
import type { ServiceDescriptor } from '../src/types.js';

const descriptor: ServiceDescriptor = {
  service: 'iam',
  version: '0.0.0',
  capabilities: ['iam.audit', 'iam.apikeys'],
};

function rec(over: Partial<CacheRecord> = {}): CacheRecord {
  return {
    version: RECORD_VERSION,
    rev: 1,
    descriptor,
    descriptorAt: 0,
    outcome: 'ok',
    outcomeAt: 0,
    reason: null,
    ...over,
  };
}

describe('isFresh', () => {
  it('honours a successful probe for FRESH_MS', () => {
    expect(isFresh(rec({ outcomeAt: 0 }), 59_000, DEFAULT_TIMINGS)).toBe(true);
    expect(isFresh(rec({ outcomeAt: 0 }), 61_000, DEFAULT_TIMINGS)).toBe(false);
  });

  it('honours a failed probe only for NEGATIVE_MS', () => {
    const failed = rec({ outcome: 'fail', reason: 'network', outcomeAt: 0 });
    expect(isFresh(failed, 9_000, DEFAULT_TIMINGS)).toBe(true);
    expect(isFresh(failed, 11_000, DEFAULT_TIMINGS)).toBe(false);
  });

  it('reads outcomeAt and NEVER descriptorAt', () => {
    // THE REGRESSION THIS TEST EXISTS FOR. A probe that failed at t=0 over a descriptor
    // fetched at t=-5000. At t=20000 the descriptor is inside FRESH_MS (25s < 60s) but the
    // OUTCOME is outside NEGATIVE_MS (20s > 10s). Reading descriptorAt here would leave a
    // down service un-probed for a full 60s and make the negative TTL dead code.
    const failed = rec({ outcome: 'fail', reason: 'network', descriptorAt: -5_000, outcomeAt: 0 });
    expect(isFresh(failed, 20_000, DEFAULT_TIMINGS)).toBe(false);
  });
});

describe('toState', () => {
  it('maps ok + descriptor to available', () => {
    expect(toState('iam', rec())).toEqual({
      state: 'available',
      service: 'iam',
      descriptor,
      capabilities: ['iam.audit', 'iam.apikeys'],
    });
  });

  it('maps fail + descriptor to degraded, keeping the capability list', () => {
    const state = toState('iam', rec({ outcome: 'fail', reason: 'network' }));
    expect(state).toEqual({
      state: 'degraded',
      service: 'iam',
      reason: 'network',
      descriptor,
      capabilities: ['iam.audit', 'iam.apikeys'],
    });
  });

  it('maps fail + no descriptor to degraded with an empty capability list', () => {
    const state = toState('iam', rec({ outcome: 'fail', reason: 'timeout', descriptor: null }));
    expect(state).toEqual({
      state: 'degraded',
      service: 'iam',
      reason: 'timeout',
      descriptor: null,
      capabilities: [],
    });
  });

  it('uses the CONFIGURED service name, never the descriptor-reported one', () => {
    // The proto MUST (service_info.proto:84-94): ServiceInfo.service is advisory and never a
    // cache key, because a misconfigured or hostile service could otherwise poison another
    // service's entry.
    const hostile = rec({ descriptor: { ...descriptor, service: 'gateway' } });
    expect(toState('iam', hostile).service).toBe('iam');
  });

  it('treats ok + null descriptor as corrupt', () => {
    expect(toState('iam', rec({ descriptor: null }))).toBeNull();
  });
});

describe('parseRecord', () => {
  it('accepts a well-formed record', () => {
    expect(parseRecord(JSON.stringify(rec()))).toEqual(rec());
  });

  it('rejects a record from a different schema version', () => {
    // A rolling upgrade has two console builds writing the same key. Without this, a renamed
    // DegradedReason poisons the cache fleet-wide for the whole 10-minute hard TTL.
    expect(parseRecord(JSON.stringify({ ...rec(), version: 2 }))).toBeNull();
  });

  it('rejects unparseable JSON', () => {
    expect(parseRecord('{not json')).toBeNull();
  });

  it('rejects a structurally wrong record', () => {
    expect(parseRecord(JSON.stringify({ version: RECORD_VERSION, rev: 'x' }))).toBeNull();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/record.test.ts
```

Expected: FAIL — cannot resolve `../src/core/record.js`.

- [ ] **Step 3: Write `src/core/record.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
import type { DegradedReason, ServiceDescriptor, ServiceState } from '../types.js';

/**
 * Bumped whenever CacheRecord's shape or DegradedReason's vocabulary changes.
 *
 * During a rolling upgrade two console builds write the same `pgs:svcinfo:<service>` key. Both
 * adapters treat a version mismatch as absent and DELETE it, the pattern SessionRecord already
 * uses. Without it a renamed reason poisons the cache fleet-wide for the full hard TTL.
 */
export const RECORD_VERSION = 1 as const;

export type CacheRecord = {
  readonly version: typeof RECORD_VERSION;
  /** Fences the write. See single-flight.ts — a compare-and-delete protects the LOCK, not the WRITE. */
  readonly rev: number;
  /** The last GOOD probe. Untouched by a failure, which is what keeps a degraded service's feature list. */
  readonly descriptor: ServiceDescriptor | null;
  readonly descriptorAt: number;
  /** The last probe ATTEMPT, successful or not. */
  readonly outcome: 'ok' | 'fail';
  readonly outcomeAt: number;
  readonly reason: DegradedReason | null;
};

export type Timings = {
  readonly negativeMs: number;
  readonly freshMs: number;
  readonly staleMs: number;
  readonly probeTimeoutMs: number;
  readonly lockWaitMs: number;
  readonly lockTtlMs: number;
};

export const DEFAULT_TIMINGS: Timings = {
  negativeMs: 10_000,
  freshMs: 60_000,
  staleMs: 600_000,
  probeTimeoutMs: 1_500,
  lockWaitMs: 2_500,
  lockTtlMs: 5_000,
};

/**
 * Whether a record may be served without re-probing.
 *
 * Reads `outcomeAt` in BOTH arms. `descriptorAt` moves only on success and is carried for
 * observability alone — using it for the `ok` arm would leave a failed probe honoured for
 * FRESH_MS whenever a stale descriptor happened to be recent, making NEGATIVE_MS dead code.
 */
export function isFresh(rec: CacheRecord, now: number, t: Timings): boolean {
  const age = now - rec.outcomeAt;
  return rec.outcome === 'ok' ? age < t.freshMs : age < t.negativeMs;
}

/**
 * Map a record to a state. Returns null when the record is internally impossible, which the
 * caller treats as corrupt: delete and re-probe.
 *
 * `service` is the CONFIGURED key, never `rec.descriptor.service` — see the proto MUST at
 * contracts/proto/paigasus/common/v1/service_info.proto:84-94.
 */
export function toState(service: string, rec: CacheRecord): ServiceState | null {
  if (rec.outcome === 'ok') {
    if (rec.descriptor === null) return null;
    return {
      state: 'available',
      service,
      descriptor: rec.descriptor,
      capabilities: rec.descriptor.capabilities,
    };
  }
  return {
    state: 'degraded',
    service,
    reason: rec.reason ?? 'network',
    descriptor: rec.descriptor,
    capabilities: rec.descriptor?.capabilities ?? [],
  };
}

function isDescriptor(v: unknown): v is ServiceDescriptor {
  if (typeof v !== 'object' || v === null) return false;
  const d = v as Record<string, unknown>;
  return (
    typeof d['service'] === 'string' &&
    typeof d['version'] === 'string' &&
    Array.isArray(d['capabilities']) &&
    d['capabilities'].every((c) => typeof c === 'string')
  );
}

/** Parse a stored record. Any doubt returns null, and the caller deletes the key. */
export function parseRecord(raw: string): CacheRecord | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof parsed !== 'object' || parsed === null) return null;
  const r = parsed as Record<string, unknown>;
  if (r['version'] !== RECORD_VERSION) return null;
  if (typeof r['rev'] !== 'number') return null;
  if (typeof r['descriptorAt'] !== 'number' || typeof r['outcomeAt'] !== 'number') return null;
  if (r['outcome'] !== 'ok' && r['outcome'] !== 'fail') return null;
  if (r['descriptor'] !== null && !isDescriptor(r['descriptor'])) return null;
  if (r['reason'] !== null && typeof r['reason'] !== 'string') return null;
  return parsed as CacheRecord;
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/record.test.ts
```

Expected: PASS, 11 tests.

- [ ] **Step 5: Commit**

```bash
git add ts/packages/paigasus-discovery/src/core/record.ts ts/packages/paigasus-discovery/tests/record.test.ts
git commit -m "feat(ts): discovery cache record, freshness on outcomeAt, and the state mapping (SMA-509)"
```

---

### Task 3: The `DescriptorCache` port and the in-memory adapter

**Files:**
- Create: `ts/packages/paigasus-discovery/src/ports/cache.ts`
- Create: `ts/packages/paigasus-discovery/src/ports/logger.ts`
- Create: `ts/packages/paigasus-discovery/src/adapters/memory-cache.ts`
- Create: `ts/packages/paigasus-discovery/src/adapters/noop-logger.ts`
- Create: `ts/packages/paigasus-discovery/tests/cache-contract.ts`
- Test: `ts/packages/paigasus-discovery/tests/memory-cache.test.ts`

**Interfaces:**
- Produces: `interface DescriptorCache`, `interface DiscoveryLogger`, `type DiscoveryEventName`, `noopLogger`, `createMemoryDescriptorCache()`, `runCacheContract(name, makeCache)`.

- [ ] **Step 1: Write the port and the logger**

`ts/packages/paigasus-discovery/src/ports/cache.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import type { CacheRecord } from '../core/record.js';

/**
 * Primitives only, no policy. The single-flight algorithm lives once in core/single-flight.ts and
 * runs identically against every adapter — the same split @paigasus/auth's SessionStore uses.
 *
 * `set` is a COMPARE-AND-SET on `rev`. This is invariant 5: a lock's compare-and-delete protects
 * the LOCK, never the WRITE. Pass `expectedRev: null` to mean "only if absent".
 * It returns false when the fence lost.
 *
 * Every method may throw. The caller degrades rather than propagating — one Redis blip must not
 * 500 every server component rendering navigation.
 */
export interface DescriptorCache {
  get(service: string): Promise<CacheRecord | null>;
  set(service: string, rec: CacheRecord, ttlMs: number, expectedRev: number | null): Promise<boolean>;
  delete(service: string): Promise<void>;
  tryAcquireLock(service: string, token: string, ttlMs: number): Promise<boolean>;
  releaseLock(service: string, token: string): Promise<void>;
  close(): Promise<void>;
}
```

`ts/packages/paigasus-discovery/src/ports/logger.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Observability is a PORT, not a detail. An operator seeing a degraded console needs to know
// WHICH service, WHY, and how often — that is the primary use case in the spec's § 1, and every
// decision point below is otherwise silent (a floated promise swallows its error, a lock timeout
// reaches only the UI).
//
// REDACTION IS THE CALLER'S CONTRACT. No event may carry the bearer token or a service base URL:
// the URL is cluster-internal topology and the token is a credential. Never pass a caught library
// error object into `fields` — node-redis embeds the DSN in its own connection errors. Extract a
// fixed message.

export type DiscoveryEventName =
  /** A probe failed. Fields: service, reason. */
  | 'discovery.probe_failed'
  /** A cold loser exhausted lockWaitMs. Fields: service. */
  | 'discovery.lock_timeout'
  /** A cache read or write threw. Fields: service, stage. */
  | 'discovery.cache_unavailable'
  /** The descriptor's own `service` disagreed with the configured key. Fields: configured, reported. */
  | 'discovery.service_mismatch'
  /** A fenced write lost its compare-and-set and was discarded. Fields: service. */
  | 'discovery.write_fenced'
  /** A stored record was unparseable or from another schema version. Fields: service. */
  | 'discovery.record_discarded';

export type DiscoveryEventFields = Readonly<Record<string, string | number | boolean>>;

export interface DiscoveryLogger {
  event(name: DiscoveryEventName, fields: DiscoveryEventFields): void;
}
```

`ts/packages/paigasus-discovery/src/adapters/noop-logger.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import type { DiscoveryLogger } from '../ports/logger.js';

/** The default. The package emits nothing unless an app opts in. */
export const noopLogger: DiscoveryLogger = { event: () => undefined };
```

- [ ] **Step 2: Write the shared contract suite**

`ts/packages/paigasus-discovery/tests/cache-contract.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// ONE contract, run against EVERY adapter. A semantic divergence between the memory adapter and
// Redis — a non-atomic lock, a fence that does not fence, a version mismatch that is served
// rather than deleted — then fails in CI instead of in production. This mirrors
// ts/packages/paigasus-auth/tests/store-contract.ts.
import { afterEach, describe, expect, it } from 'vitest';
import { RECORD_VERSION, type CacheRecord } from '../src/core/record.js';
import type { DescriptorCache } from '../src/ports/cache.js';

function rec(over: Partial<CacheRecord> = {}): CacheRecord {
  return {
    version: RECORD_VERSION,
    rev: 1,
    descriptor: { service: 'iam', version: '0.0.0', capabilities: ['iam.audit'] },
    descriptorAt: 0,
    outcome: 'ok',
    outcomeAt: 0,
    reason: null,
    ...over,
  };
}

export function runCacheContract(name: string, makeCache: () => Promise<DescriptorCache>): void {
  describe(`DescriptorCache contract: ${name}`, () => {
    let cache: DescriptorCache;
    afterEach(async () => {
      await cache?.close();
    });

    it('returns null for an unknown service', async () => {
      cache = await makeCache();
      expect(await cache.get('iam')).toBeNull();
    });

    it('stores and reads back a record', async () => {
      cache = await makeCache();
      expect(await cache.set('iam', rec(), 60_000, null)).toBe(true);
      expect(await cache.get('iam')).toEqual(rec());
    });

    it('refuses an insert when the key already exists (expectedRev null)', async () => {
      cache = await makeCache();
      await cache.set('iam', rec(), 60_000, null);
      expect(await cache.set('iam', rec({ rev: 9 }), 60_000, null)).toBe(false);
    });

    it('fences a write on rev', async () => {
      cache = await makeCache();
      await cache.set('iam', rec({ rev: 1 }), 60_000, null);
      expect(await cache.set('iam', rec({ rev: 2 }), 60_000, 1)).toBe(true);
      // A writer still holding rev 1 has lost and must be rejected.
      expect(await cache.set('iam', rec({ rev: 2, outcome: 'fail', reason: 'network' }), 60_000, 1)).toBe(false);
      expect((await cache.get('iam'))?.outcome).toBe('ok');
    });

    it('deletes', async () => {
      cache = await makeCache();
      await cache.set('iam', rec(), 60_000, null);
      await cache.delete('iam');
      expect(await cache.get('iam')).toBeNull();
    });

    it('keeps each service in its own entry', async () => {
      cache = await makeCache();
      await cache.set('iam', rec(), 60_000, null);
      await cache.set('gateway', rec({ rev: 5 }), 60_000, null);
      expect((await cache.get('iam'))?.rev).toBe(1);
      expect((await cache.get('gateway'))?.rev).toBe(5);
    });

    it('grants a lock to exactly one holder', async () => {
      cache = await makeCache();
      expect(await cache.tryAcquireLock('iam', 'token-a', 5_000)).toBe(true);
      expect(await cache.tryAcquireLock('iam', 'token-b', 5_000)).toBe(false);
    });

    it('releases a lock it holds', async () => {
      cache = await makeCache();
      await cache.tryAcquireLock('iam', 'token-a', 5_000);
      await cache.releaseLock('iam', 'token-a');
      expect(await cache.tryAcquireLock('iam', 'token-b', 5_000)).toBe(true);
    });

    it('refuses to release a lock held by someone else', async () => {
      // Compare-and-delete. A holder whose TTL expired must not release the NEXT holder's lock.
      cache = await makeCache();
      await cache.tryAcquireLock('iam', 'token-a', 5_000);
      await cache.releaseLock('iam', 'token-b');
      expect(await cache.tryAcquireLock('iam', 'token-c', 5_000)).toBe(false);
    });

    it('locks one service without locking another', async () => {
      cache = await makeCache();
      await cache.tryAcquireLock('iam', 'token-a', 5_000);
      expect(await cache.tryAcquireLock('gateway', 'token-b', 5_000)).toBe(true);
    });

    it('discards a record written by a different schema version', async () => {
      cache = await makeCache();
      await cache.set('iam', rec(), 60_000, null);
      await cache.writeRawForTest?.('iam', JSON.stringify({ ...rec(), version: 99 }));
      expect(await cache.get('iam')).toBeNull();
    });
  });
}

declare module '../src/ports/cache.js' {
  interface DescriptorCache {
    /** Test-only seam so the contract can plant a foreign-version record. Adapters implement it. */
    writeRawForTest?(service: string, raw: string): Promise<void>;
  }
}
```

- [ ] **Step 3: Write the memory-adapter test**

`ts/packages/paigasus-discovery/tests/memory-cache.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { createMemoryDescriptorCache } from '../src/adapters/memory-cache.js';
import { runCacheContract } from './cache-contract.js';

runCacheContract('memory', async () => createMemoryDescriptorCache());
```

- [ ] **Step 4: Run to verify it fails**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/memory-cache.test.ts
```

Expected: FAIL — cannot resolve `../src/adapters/memory-cache.js`.

- [ ] **Step 5: Write the memory adapter**

`ts/packages/paigasus-discovery/src/adapters/memory-cache.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { parseRecord, type CacheRecord } from '../core/record.js';
import type { DescriptorCache } from '../ports/cache.js';

type Entry = { readonly raw: string; readonly expiresAt: number };
type Lock = { readonly token: string; readonly expiresAt: number };

/**
 * A single-process cache for dev and for the fast test tier.
 *
 * It stores the SERIALIZED record and reads it back through `parseRecord`, exactly as the Redis
 * adapter does. Holding the object directly would let the memory adapter pass a contract case the
 * Redis adapter fails — the divergence the shared contract suite exists to prevent.
 */
export function createMemoryDescriptorCache(now: () => number = Date.now): DescriptorCache {
  const entries = new Map<string, Entry>();
  const locks = new Map<string, Lock>();

  const live = (service: string): Entry | null => {
    const e = entries.get(service);
    if (e === undefined) return null;
    if (now() >= e.expiresAt) {
      entries.delete(service);
      return null;
    }
    return e;
  };

  return {
    get(service) {
      const e = live(service);
      if (e === null) return Promise.resolve(null);
      const parsed = parseRecord(e.raw);
      if (parsed === null) {
        entries.delete(service);
        return Promise.resolve(null);
      }
      return Promise.resolve(parsed);
    },

    set(service, rec: CacheRecord, ttlMs, expectedRev) {
      const e = live(service);
      if (expectedRev === null) {
        if (e !== null) return Promise.resolve(false);
      } else {
        if (e === null) return Promise.resolve(false);
        const cur = parseRecord(e.raw);
        if (cur === null || cur.rev !== expectedRev) return Promise.resolve(false);
      }
      entries.set(service, { raw: JSON.stringify(rec), expiresAt: now() + ttlMs });
      return Promise.resolve(true);
    },

    delete(service) {
      entries.delete(service);
      return Promise.resolve();
    },

    tryAcquireLock(service, token, ttlMs) {
      const l = locks.get(service);
      if (l !== undefined && now() < l.expiresAt) return Promise.resolve(false);
      locks.set(service, { token, expiresAt: now() + ttlMs });
      return Promise.resolve(true);
    },

    releaseLock(service, token) {
      // Compare-and-delete: a holder whose TTL expired must not release the next holder's lock.
      if (locks.get(service)?.token === token) locks.delete(service);
      return Promise.resolve();
    },

    close() {
      entries.clear();
      locks.clear();
      return Promise.resolve();
    },

    writeRawForTest(service, raw) {
      entries.set(service, { raw, expiresAt: now() + 60_000 });
      return Promise.resolve();
    },
  };
}
```

- [ ] **Step 6: Run to verify it passes**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/memory-cache.test.ts
```

Expected: PASS, 11 contract cases.

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-discovery/src ts/packages/paigasus-discovery/tests
git commit -m "feat(ts): DescriptorCache port, logger port, and the in-memory adapter (SMA-509)"
```

---

### Task 4: The probe

**Files:**
- Create: `ts/packages/paigasus-discovery/src/core/reasons.ts`
- Create: `ts/packages/paigasus-discovery/src/probe.ts`
- Test: `ts/packages/paigasus-discovery/tests/probe.test.ts`

**Interfaces:**
- Produces: `reasonForStatus(status)`, `reasonForThrown(err)`, `type ProbeOutcome`, `probeService(opts)`.
- `ProbeOutcome` is `{ ok: true; descriptor: ServiceDescriptor } | { ok: false; reason: DegradedReason }`.

- [ ] **Step 1: Write the failing test**

`ts/packages/paigasus-discovery/tests/probe.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from 'vitest';
import { probeService } from '../src/probe.js';

const OK_BODY = JSON.stringify({ service: 'iam', version: '1.2.3', capabilities: ['iam.audit'] });

function res(body: string, init: { status?: number; contentType?: string | null } = {}): Response {
  const headers = new Headers();
  if (init.contentType !== null) headers.set('content-type', init.contentType ?? 'application/json');
  return new Response(body, { status: init.status ?? 200, headers });
}

function probeWith(fetchImpl: typeof globalThis.fetch, token = 'tok') {
  return probeService({
    baseUrl: 'http://iam:8080',
    token,
    timeoutMs: 1_000,
    maxBytes: 64 * 1024,
    fetch: fetchImpl,
  });
}

describe('probeService', () => {
  it('returns the descriptor on a well-formed 200', async () => {
    const out = await probeWith(async () => res(OK_BODY));
    expect(out).toEqual({
      ok: true,
      descriptor: { service: 'iam', version: '1.2.3', capabilities: ['iam.audit'] },
    });
  });

  it('requests /v1/service-info with a bearer token and refuses redirects', async () => {
    const fetchImpl = vi.fn<typeof globalThis.fetch>(async () => res(OK_BODY));
    await probeWith(fetchImpl);
    const [url, init] = fetchImpl.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('http://iam:8080/v1/service-info');
    expect(new Headers(init.headers).get('authorization')).toBe('Bearer tok');
    // fetch follows redirects by default; following one would send the user's bearer token
    // wherever a compromised or misconfigured service points.
    expect(init.redirect).toBe('error');
  });

  it('defaults capabilities to an empty array when the field is absent', async () => {
    const out = await probeWith(async () => res(JSON.stringify({ service: 'iam', version: '1.0.0' })));
    expect(out).toEqual({ ok: true, descriptor: { service: 'iam', version: '1.0.0', capabilities: [] } });
  });

  it('drops unknown fields rather than failing', async () => {
    const body = JSON.stringify({ service: 'iam', version: '1.0.0', capabilities: [], futureField: 42 });
    const out = await probeWith(async () => res(body));
    expect(out).toEqual({ ok: true, descriptor: { service: 'iam', version: '1.0.0', capabilities: [] } });
  });

  it('accepts a content-type in ANY case, with parameters', async () => {
    // An externally supplied header must be compared case-insensitively. Every fixture here
    // varies the case deliberately: a lowercase-only fixture set is how a case-sensitive check
    // survives review.
    for (const ct of ['application/json', 'Application/JSON', 'APPLICATION/JSON; charset=utf-8']) {
      const out = await probeWith(async () => res(OK_BODY, { contentType: ct }));
      expect(out, ct).toMatchObject({ ok: true });
    }
  });

  it.each([
    [401, 'unauthorized'],
    [403, 'unauthorized'],
    [404, 'not-implemented'],
    [500, 'server-error'],
    [502, 'server-error'],
    [418, 'server-error'],
  ])('maps status %i to %s', async (status, reason) => {
    expect(await probeWith(async () => res('{}', { status }))).toEqual({ ok: false, reason });
  });

  it('rejects a 200 whose content-type is not JSON', async () => {
    const out = await probeWith(async () => res('<html>', { contentType: 'text/html' }));
    expect(out).toEqual({ ok: false, reason: 'bad-response' });
  });

  it('rejects a 200 with no content-type at all', async () => {
    expect(await probeWith(async () => res(OK_BODY, { contentType: null }))).toEqual({
      ok: false,
      reason: 'bad-response',
    });
  });

  it('rejects a 200 whose body is not a descriptor', async () => {
    expect(await probeWith(async () => res(JSON.stringify({ nope: true })))).toEqual({
      ok: false,
      reason: 'bad-response',
    });
  });

  it('rejects an oversized body', async () => {
    const huge = JSON.stringify({ service: 'iam', version: 'x'.repeat(200_000), capabilities: [] });
    const out = await probeService({
      baseUrl: 'http://iam:8080',
      token: 'tok',
      timeoutMs: 1_000,
      maxBytes: 1_024,
      fetch: async () => res(huge),
    });
    expect(out).toEqual({ ok: false, reason: 'bad-response' });
  });

  it('maps an abort to timeout', async () => {
    const out = await probeWith(async () => {
      throw Object.assign(new Error('The operation was aborted'), { name: 'TimeoutError' });
    });
    expect(out).toEqual({ ok: false, reason: 'timeout' });
  });

  it('maps a fetch TypeError to network', async () => {
    const out = await probeWith(async () => {
      throw new TypeError('fetch failed');
    });
    expect(out).toEqual({ ok: false, reason: 'network' });
  });

  it('never probes without a token', async () => {
    const fetchImpl = vi.fn<typeof globalThis.fetch>(async () => res(OK_BODY));
    expect(await probeWith(fetchImpl, '   ')).toEqual({ ok: false, reason: 'unauthorized' });
    expect(fetchImpl).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/probe.test.ts
```

Expected: FAIL — cannot resolve `../src/probe.js`.

- [ ] **Step 3: Write `src/core/reasons.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
import type { DegradedReason } from '../types.js';

/**
 * HTTP status to reason.
 *
 * Owned here, not imported from @paigasus/sdk: that package's `Presentation` union
 * ('relogin' | 'forbidden' | 'not-found' | 'degraded' | ...) cannot express any of these, its
 * HTTP table has no 500 row, and it deliberately has no 2xx row (spec F8). Taking the dependency
 * would also give @paigasus/app-shell a transitive path to the SDK, which its eslint boundary
 * block bans.
 */
export function reasonForStatus(status: number): DegradedReason {
  if (status === 401 || status === 403) return 'unauthorized';
  // 404 means a service predating the descriptor, or one whose capability routes are unmounted —
  // ADR-0020 A2 makes those deliberately indistinguishable.
  if (status === 404) return 'not-implemented';
  return 'server-error';
}

/** A thrown fetch failure to a reason. */
export function reasonForThrown(err: unknown): DegradedReason {
  if (err instanceof Error && (err.name === 'TimeoutError' || err.name === 'AbortError')) {
    return 'timeout';
  }
  // `fetch` reports connection refused, DNS failure, TLS failure AND a refused redirect as
  // TypeError. All are 'network' for our purposes.
  return 'network';
}
```

- [ ] **Step 4: Write `src/probe.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
import { reasonForStatus, reasonForThrown } from './core/reasons.js';
import type { DegradedReason, ServiceDescriptor } from './types.js';

export type ProbeOutcome =
  | { readonly ok: true; readonly descriptor: ServiceDescriptor }
  | { readonly ok: false; readonly reason: DegradedReason };

export type ProbeOptions = {
  readonly baseUrl: string;
  readonly token: string;
  readonly timeoutMs: number;
  readonly maxBytes: number;
  readonly fetch: typeof globalThis.fetch;
};

/** The path both services serve. IAM also has a gRPC RPC; the gateway has no gRPC server at all. */
export const SERVICE_INFO_PATH = '/v1/service-info';

function isJsonContentType(value: string | null): boolean {
  if (value === null) return false;
  // Case-insensitive by contract: this header is supplied by another system.
  const essence = value.split(';', 1)[0]?.trim().toLowerCase();
  return essence === 'application/json';
}

function toDescriptor(body: unknown): ServiceDescriptor | null {
  if (typeof body !== 'object' || body === null) return null;
  const b = body as Record<string, unknown>;
  if (typeof b['service'] !== 'string' || typeof b['version'] !== 'string') return null;
  const caps = b['capabilities'];
  // Both services always send `capabilities`, as [] when empty. Tolerate its absence anyway:
  // an older build is exactly what this whole mechanism exists to cope with.
  if (caps !== undefined && (!Array.isArray(caps) || !caps.every((c) => typeof c === 'string'))) {
    return null;
  }
  // Unknown fields are dropped HERE. That is decision 6's "unknown key -> ignore", enforced at
  // the boundary rather than by every downstream reader.
  return {
    service: b['service'],
    version: b['version'],
    capabilities: caps === undefined ? [] : (caps as string[]),
  };
}

async function readCapped(response: Response, maxBytes: number): Promise<string | null> {
  const declared = response.headers.get('content-length');
  if (declared !== null && Number(declared) > maxBytes) return null;
  const body = response.body;
  if (body === null) return '';
  const reader = body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    // Cap enforced WHILE reading, so a server that lies about content-length cannot make us
    // buffer an arbitrary body inside the timeout window on a fast internal link.
    if (total > maxBytes) {
      await reader.cancel();
      return null;
    }
    chunks.push(value);
  }
  return new TextDecoder().decode(await new Blob(chunks).arrayBuffer());
}

/**
 * One probe of one service. Never throws.
 *
 * A 401/403 is returned to the caller and — critically — is NEVER cached by the resolver: the
 * descriptor is caller-independent but the AUTH OUTCOME is not, so writing it into the shared
 * entry would let one user's expired cookie disable navigation for the whole deployment.
 */
export async function probeService(opts: ProbeOptions): Promise<ProbeOutcome> {
  if (opts.token.trim() === '') return { ok: false, reason: 'unauthorized' };

  let response: Response;
  try {
    response = await opts.fetch(`${opts.baseUrl}${SERVICE_INFO_PATH}`, {
      method: 'GET',
      headers: { authorization: `Bearer ${opts.token}`, accept: 'application/json' },
      // fetch follows redirects by default. Following one would forward the user's bearer token
      // to wherever a compromised or misconfigured service points.
      redirect: 'error',
      signal: AbortSignal.timeout(opts.timeoutMs),
    });
  } catch (err) {
    return { ok: false, reason: reasonForThrown(err) };
  }

  if (!response.ok) return { ok: false, reason: reasonForStatus(response.status) };
  if (!isJsonContentType(response.headers.get('content-type'))) {
    return { ok: false, reason: 'bad-response' };
  }

  let raw: string | null;
  try {
    raw = await readCapped(response, opts.maxBytes);
  } catch {
    return { ok: false, reason: 'network' };
  }
  if (raw === null) return { ok: false, reason: 'bad-response' };

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return { ok: false, reason: 'bad-response' };
  }

  const descriptor = toDescriptor(parsed);
  return descriptor === null ? { ok: false, reason: 'bad-response' } : { ok: true, descriptor };
}
```

- [ ] **Step 5: Run to verify it passes**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/probe.test.ts
```

Expected: PASS, 19 tests (the `it.each` contributes 6).

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-discovery/src ts/packages/paigasus-discovery/tests
git commit -m "feat(ts): the service-info probe with redirect, content-type and size guards (SMA-509)"
```

---

### Task 5: Single-flight resolution with stale-while-revalidate

This task carries AC2 and AC3's fast tier. It is the largest task; do not split the algorithm from its tests.

**Files:**
- Create: `ts/packages/paigasus-discovery/src/core/single-flight.ts`
- Test: `ts/packages/paigasus-discovery/tests/single-flight.test.ts`

**Interfaces:**
- Consumes: `DescriptorCache`, `DiscoveryLogger`, `Timings`, `isFresh`, `toState`, `parseRecord`, `ProbeOutcome`.
- Produces: `resolveService(deps, service, token): Promise<ServiceState>` where `deps` is `{ cache, logger, timings, now, probe, waitUntil?, sleep? }` and `probe` is `(service: string, token: string) => Promise<ProbeOutcome>`.

- [ ] **Step 1: Write the failing test**

`ts/packages/paigasus-discovery/tests/single-flight.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from 'vitest';
import { createMemoryDescriptorCache } from '../src/adapters/memory-cache.js';
import { noopLogger } from '../src/adapters/noop-logger.js';
import { DEFAULT_TIMINGS, RECORD_VERSION, type CacheRecord } from '../src/core/record.js';
import { resolveService, type ResolveDeps } from '../src/core/single-flight.js';
import type { ProbeOutcome } from '../src/probe.js';
import type { DescriptorCache } from '../src/ports/cache.js';
import type { ServiceDescriptor } from '../src/types.js';

const descriptor: ServiceDescriptor = { service: 'iam', version: '1.0.0', capabilities: ['iam.audit'] };

function deps(over: Partial<ResolveDeps> = {}): ResolveDeps {
  return {
    cache: createMemoryDescriptorCache(),
    logger: noopLogger,
    timings: DEFAULT_TIMINGS,
    now: () => Date.now(),
    probe: async (): Promise<ProbeOutcome> => ({ ok: true, descriptor }),
    ...over,
  };
}

function storedRecord(over: Partial<CacheRecord> = {}): CacheRecord {
  return {
    version: RECORD_VERSION,
    rev: 1,
    descriptor,
    descriptorAt: 0,
    outcome: 'ok',
    outcomeAt: 0,
    reason: null,
    ...over,
  };
}

describe('cold resolution', () => {
  it('probes and returns available', async () => {
    const state = await resolveService(deps(), 'iam', 'tok');
    expect(state).toMatchObject({ state: 'available', service: 'iam', capabilities: ['iam.audit'] });
  });

  it('caches a successful probe', async () => {
    const cache = createMemoryDescriptorCache();
    const probe = vi.fn(async (): Promise<ProbeOutcome> => ({ ok: true, descriptor }));
    await resolveService(deps({ cache, probe }), 'iam', 'tok');
    await resolveService(deps({ cache, probe }), 'iam', 'tok');
    expect(probe).toHaveBeenCalledTimes(1);
  });

  it('returns degraded and caches the failure', async () => {
    const cache = createMemoryDescriptorCache();
    const probe = vi.fn(async (): Promise<ProbeOutcome> => ({ ok: false, reason: 'network' }));
    const state = await resolveService(deps({ cache, probe }), 'iam', 'tok');
    expect(state).toEqual({
      state: 'degraded',
      service: 'iam',
      reason: 'network',
      descriptor: null,
      capabilities: [],
    });
    expect(await cache.get('iam')).toMatchObject({ outcome: 'fail', reason: 'network' });
  });

  it('NEVER caches a 401/403 outcome', async () => {
    // The descriptor is caller-independent; the auth outcome is not. Caching it would let one
    // user's expired cookie disable navigation for every user in the deployment for the
    // negative TTL — self-perpetuating on a low-traffic deployment.
    const cache = createMemoryDescriptorCache();
    const probe = async (): Promise<ProbeOutcome> => ({ ok: false, reason: 'unauthorized' });
    const state = await resolveService(deps({ cache, probe }), 'iam', 'tok');
    expect(state).toMatchObject({ state: 'degraded', reason: 'unauthorized' });
    expect(await cache.get('iam')).toBeNull();
  });
});

describe('fresh and stale', () => {
  it('serves a fresh record without probing', async () => {
    const cache = createMemoryDescriptorCache();
    await cache.set('iam', storedRecord({ outcomeAt: 0 }), DEFAULT_TIMINGS.staleMs, null);
    const probe = vi.fn(async (): Promise<ProbeOutcome> => ({ ok: true, descriptor }));
    const state = await resolveService(deps({ cache, probe, now: () => 30_000 }), 'iam', 'tok');
    expect(state).toMatchObject({ state: 'available' });
    expect(probe).not.toHaveBeenCalled();
  });

  it('serves a fresh FAILURE without probing, for the negative TTL only', async () => {
    const cache = createMemoryDescriptorCache();
    await cache.set(
      'iam',
      storedRecord({ outcome: 'fail', reason: 'network', outcomeAt: 0 }),
      DEFAULT_TIMINGS.staleMs,
      null,
    );
    const probe = vi.fn(async (): Promise<ProbeOutcome> => ({ ok: true, descriptor }));
    expect(await resolveService(deps({ cache, probe, now: () => 5_000 }), 'iam', 'tok')).toMatchObject({
      state: 'degraded',
    });
    expect(probe).not.toHaveBeenCalled();
  });

  // AC2. NO WALL-CLOCK ASSERTION: a clock threshold passes for an implementation that awaits a
  // 1ms probe and fails on a loaded CI runner. A probe that NEVER settles makes an
  // await-the-probe implementation hang and fail deterministically at the suite timeout.
  it('AC2: a stale record returns immediately even when the probe never settles', async () => {
    const cache = createMemoryDescriptorCache();
    await cache.set('iam', storedRecord({ outcomeAt: 0 }), DEFAULT_TIMINGS.staleMs, null);
    let probeCalled = false;
    const probe = (): Promise<ProbeOutcome> => {
      probeCalled = true;
      return new Promise<ProbeOutcome>(() => {
        /* never settles */
      });
    };
    const state = await resolveService(deps({ cache, probe, now: () => 120_000 }), 'iam', 'tok');
    expect(state).toMatchObject({ state: 'available', capabilities: ['iam.audit'] });
    // The descriptor came from the cache, by identity, not from a fresh probe.
    expect((state as { descriptor: ServiceDescriptor }).descriptor).toEqual(descriptor);
    // ...and revalidation WAS attempted, so this is stale-while-revalidate and not stale-only.
    expect(probeCalled).toBe(true);
  });

  it('hands the background revalidation to waitUntil when given one', async () => {
    const cache = createMemoryDescriptorCache();
    await cache.set('iam', storedRecord({ outcomeAt: 0 }), DEFAULT_TIMINGS.staleMs, null);
    const handed: Promise<unknown>[] = [];
    await resolveService(
      deps({ cache, now: () => 120_000, waitUntil: (p) => handed.push(p) }),
      'iam',
      'tok',
    );
    expect(handed).toHaveLength(1);
    await handed[0];
    expect((await cache.get('iam'))?.rev).toBe(2);
  });
});

describe('AC3: single-flight', () => {
  it('a fast probe serves many concurrent cold callers with exactly one probe', async () => {
    const cache = createMemoryDescriptorCache();
    let calls = 0;
    const probe = async (): Promise<ProbeOutcome> => {
      calls += 1;
      await new Promise((r) => setTimeout(r, 5));
      return { ok: true, descriptor };
    };
    const d = deps({ cache, probe });
    const states = await Promise.all(
      Array.from({ length: 12 }, () => resolveService(d, 'iam', 'tok')),
    );
    expect(calls).toBe(1);
    for (const s of states) expect(s).toMatchObject({ state: 'available' });
  });

  // THE CASE THAT ACTUALLY OBSERVES INVARIANT 2. With a fast probe the winner finishes long
  // before any loser reaches its deadline, so the fallback branch never executes and the counter
  // reads 1 even for an implementation that DOES fall back to probing. Only a probe that
  // outlasts lockWaitMs drives a loser down that path.
  it('a never-settling probe still yields exactly one probe, and losers report timeout', async () => {
    const cache = createMemoryDescriptorCache();
    let calls = 0;
    const probe = (): Promise<ProbeOutcome> => {
      calls += 1;
      return new Promise<ProbeOutcome>(() => {
        /* never settles */
      });
    };
    const d = deps({
      cache,
      probe,
      timings: { ...DEFAULT_TIMINGS, lockWaitMs: 60, probeTimeoutMs: 30, lockTtlMs: 5_000 },
    });
    const states = await Promise.all(
      Array.from({ length: 8 }, () => resolveService(d, 'iam', 'tok')),
    );
    expect(calls).toBe(1);
    const losers = states.filter((s) => s.state === 'degraded');
    expect(losers.length).toBeGreaterThanOrEqual(7);
    for (const s of losers) expect(s).toMatchObject({ reason: 'timeout' });
  });

  it('a loser performs one final cache read before degrading', async () => {
    // If the winner wrote just as the deadline expired, the loser must see it rather than
    // reporting a false outage.
    const cache = createMemoryDescriptorCache();
    await cache.tryAcquireLock('iam', 'someone-else', 5_000);
    const d = deps({ cache, timings: { ...DEFAULT_TIMINGS, lockWaitMs: 40 } });
    const pending = resolveService(d, 'iam', 'tok');
    await cache.set('iam', storedRecord({ outcomeAt: Date.now() }), DEFAULT_TIMINGS.staleMs, null);
    expect(await pending).toMatchObject({ state: 'available' });
  });
});

describe('failure handling', () => {
  it('degrades with cache-unavailable when the cache throws, never propagating', async () => {
    const broken: DescriptorCache = {
      get: () => Promise.reject(new Error('redis down')),
      set: () => Promise.reject(new Error('redis down')),
      delete: () => Promise.resolve(),
      tryAcquireLock: () => Promise.reject(new Error('redis down')),
      releaseLock: () => Promise.resolve(),
      close: () => Promise.resolve(),
    };
    const state = await resolveService(deps({ cache: broken }), 'iam', 'tok');
    expect(state).toEqual({
      state: 'degraded',
      service: 'iam',
      reason: 'cache-unavailable',
      descriptor: null,
      capabilities: [],
    });
  });

  it('discards a write whose fence was lost rather than clobbering a newer record', async () => {
    // Invariant 5. A background probe with no wall-clock bound can resolve long after a newer
    // probe has already written; last-writer-wins would let an OLD failure mask a healthy
    // service until the hard TTL.
    const cache = createMemoryDescriptorCache();
    const events: string[] = [];
    const logger = { event: (n: string) => events.push(n) };
    await cache.set('iam', storedRecord({ rev: 7, outcomeAt: 0 }), DEFAULT_TIMINGS.staleMs, null);

    let release!: (o: ProbeOutcome) => void;
    const probe = (): Promise<ProbeOutcome> =>
      new Promise<ProbeOutcome>((resolve) => {
        release = resolve;
      });

    const d = deps({ cache, probe, logger, now: () => 120_000 });
    const handed: Promise<unknown>[] = [];
    await resolveService({ ...d, waitUntil: (p) => handed.push(p) }, 'iam', 'tok');

    // A newer writer lands while the slow probe is still in flight.
    await cache.set('iam', storedRecord({ rev: 8, outcomeAt: 119_000 }), DEFAULT_TIMINGS.staleMs, 7);
    release({ ok: false, reason: 'network' });
    await handed[0];

    const final = await cache.get('iam');
    expect(final?.rev).toBe(8);
    expect(final?.outcome).toBe('ok');
    expect(events).toContain('discovery.write_fenced');
  });

  it('deletes and re-probes a corrupt record', async () => {
    const cache = createMemoryDescriptorCache();
    await cache.writeRawForTest?.('iam', JSON.stringify({ ...storedRecord(), version: 99 }));
    const probe = vi.fn(async (): Promise<ProbeOutcome> => ({ ok: true, descriptor }));
    const state = await resolveService(deps({ cache, probe }), 'iam', 'tok');
    expect(state).toMatchObject({ state: 'available' });
    expect(probe).toHaveBeenCalledTimes(1);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/single-flight.test.ts
```

Expected: FAIL — cannot resolve `../src/core/single-flight.js`.

- [ ] **Step 3: Write `src/core/single-flight.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Stale-while-revalidate with a single-flight lock, modelled on
// ts/packages/paigasus-auth/src/core/single-flight.ts. Its five invariants are preserved:
//
//  1. DOUBLE-CHECK after acquiring the lock. Another holder may have finished between the failed
//     read and the successful acquire.
//  2. A WAITER NEVER PROBES on timeout. It reports degraded/timeout instead. A fallback probe
//     would defeat the whole mechanism under exactly the load it exists for.
//  3. UNIQUE LOCK TOKEN plus compare-and-delete release, so a holder whose TTL expired cannot
//     release the next holder's lock.
//  4. RELEASE IN `finally`, and that release is itself wrapped in try/catch so a store error at
//     release time cannot mask the probe's real outcome.
//  5. THE WRITE IS FENCED on `rev`. A compare-and-delete protects the LOCK, never the WRITE.
//     This matters more here than in auth: a background revalidation has NO wall-clock bound
//     (AbortSignal.timeout bounds the network wait only, not parsing, not the write, not an
//     event-loop stall), so an old probe could otherwise overwrite a newer success with its own
//     stale failure and mask a healthy service until the hard TTL.

import { isFresh, toState, type CacheRecord, type Timings } from './record.js';
import { RECORD_VERSION } from './record.js';
import type { ProbeOutcome } from '../probe.js';
import type { DescriptorCache } from '../ports/cache.js';
import type { DiscoveryLogger } from '../ports/logger.js';
import type { DegradedReason, ServiceState } from '../types.js';

export type ResolveDeps = {
  readonly cache: DescriptorCache;
  readonly logger: DiscoveryLogger;
  readonly timings: Timings;
  readonly now: () => number;
  readonly probe: (service: string, token: string) => Promise<ProbeOutcome>;
  readonly waitUntil?: (p: Promise<unknown>) => void;
  readonly sleep?: (ms: number) => Promise<void>;
};

const defaultSleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

/** Backoff with jitter, capped. Mirrors auth's waiter. */
const backoff = (attempt: number): number => Math.min(10 * 2 ** attempt, 100) * (0.5 + Math.random());

function newLockToken(): string {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

function degraded(service: string, reason: DegradedReason, rec: CacheRecord | null): ServiceState {
  return {
    state: 'degraded',
    service,
    reason,
    descriptor: rec?.descriptor ?? null,
    capabilities: rec?.descriptor?.capabilities ?? [],
  };
}

/** A 401/403 is about the CALLER, not the service, so it must never reach the shared entry. */
function isCallerScoped(reason: DegradedReason): boolean {
  return reason === 'unauthorized';
}

function recordFor(outcome: ProbeOutcome, prev: CacheRecord | null, now: number): CacheRecord {
  const rev = (prev?.rev ?? 0) + 1;
  if (outcome.ok) {
    return {
      version: RECORD_VERSION,
      rev,
      descriptor: outcome.descriptor,
      descriptorAt: now,
      outcome: 'ok',
      outcomeAt: now,
      reason: null,
    };
  }
  return {
    version: RECORD_VERSION,
    rev,
    // The last GOOD descriptor survives a failure. This is what lets a degraded service render
    // its known feature list, each item disabled with a reason, instead of collapsing the nav.
    descriptor: prev?.descriptor ?? null,
    descriptorAt: prev?.descriptorAt ?? 0,
    outcome: 'fail',
    outcomeAt: now,
    reason: outcome.reason,
  };
}

async function readRecord(deps: ResolveDeps, service: string): Promise<CacheRecord | null> {
  const rec = await deps.cache.get(service);
  if (rec === null) return null;
  // toState returns null for an internally impossible record (ok with no descriptor). Treat it
  // as corrupt: delete and re-probe.
  if (toState(service, rec) === null) {
    deps.logger.event('discovery.record_discarded', { service });
    await deps.cache.delete(service);
    return null;
  }
  return rec;
}

/** Probe under the lock and write the result, fenced. Returns the resulting state. */
async function probeAndStore(
  deps: ResolveDeps,
  service: string,
  token: string,
  lockToken: string,
): Promise<ServiceState> {
  try {
    // Invariant 1: double-check. A winner may have written between our read and our acquire.
    const fresh = await readRecord(deps, service);
    if (fresh !== null && isFresh(fresh, deps.now(), deps.timings)) {
      return toState(service, fresh) ?? degraded(service, 'network', fresh);
    }

    const startedAt = deps.now();
    const outcome = await deps.probe(service, token);

    if (!outcome.ok) {
      deps.logger.event('discovery.probe_failed', { service, reason: outcome.reason });
      if (isCallerScoped(outcome.reason)) {
        return degraded(service, outcome.reason, fresh);
      }
    } else if (outcome.descriptor.service !== service) {
      // The proto MUST: the descriptor's own `service` is advisory and never a cache key. A
      // mismatch is worth logging and nothing more.
      deps.logger.event('discovery.service_mismatch', {
        configured: service,
        reported: outcome.descriptor.service,
      });
    }

    const prev = await readRecord(deps, service);
    // Invariant 5, second half: a write whose probe STARTED before the stored outcome is stale
    // by construction. Discard without attempting.
    if (prev !== null && prev.outcomeAt > startedAt) {
      deps.logger.event('discovery.write_fenced', { service });
      return toState(service, prev) ?? degraded(service, 'network', prev);
    }

    const next = recordFor(outcome, prev, deps.now());
    const written = await deps.cache.set(service, next, deps.timings.staleMs, prev?.rev ?? null);
    if (!written) {
      deps.logger.event('discovery.write_fenced', { service });
      const winner = await readRecord(deps, service);
      if (winner !== null) return toState(service, winner) ?? degraded(service, 'network', winner);
    }
    return toState(service, next) ?? degraded(service, 'network', next);
  } finally {
    // Invariant 4: release in `finally`, and wrap the release itself so a store error here
    // cannot mask the real outcome above.
    try {
      await deps.cache.releaseLock(service, lockToken);
    } catch {
      deps.logger.event('discovery.cache_unavailable', { service, stage: 'release_lock' });
    }
  }
}

export async function resolveService(
  deps: ResolveDeps,
  service: string,
  token: string,
): Promise<ServiceState> {
  const sleep = deps.sleep ?? defaultSleep;

  let rec: CacheRecord | null;
  try {
    rec = await readRecord(deps, service);
  } catch {
    // Never propagate. One Redis blip must not 500 every server component rendering navigation.
    deps.logger.event('discovery.cache_unavailable', { service, stage: 'read' });
    return degraded(service, 'cache-unavailable', null);
  }

  if (rec !== null && isFresh(rec, deps.now(), deps.timings)) {
    return toState(service, rec) ?? degraded(service, 'network', rec);
  }

  if (rec !== null) {
    // STALE: serve immediately, revalidate without awaiting. This is AC2.
    const stale = toState(service, rec) ?? degraded(service, 'network', rec);
    const revalidate = (async (): Promise<void> => {
      const lockToken = newLockToken();
      // If the lock is held, ANOTHER caller is already revalidating; a second waiter buys
      // nothing. A leaked lock is bounded by lockTtlMs, not by the hard TTL.
      if (!(await deps.cache.tryAcquireLock(service, lockToken, deps.timings.lockTtlMs))) return;
      await probeAndStore(deps, service, token, lockToken);
    })().catch(() => {
      deps.logger.event('discovery.cache_unavailable', { service, stage: 'revalidate' });
    });

    if (deps.waitUntil !== undefined) deps.waitUntil(revalidate);
    return stale;
  }

  // COLD: take the lock and probe, or wait for whoever holds it.
  const lockToken = newLockToken();
  const deadline = deps.now() + deps.timings.lockWaitMs;
  for (let attempt = 0; ; attempt += 1) {
    let acquired: boolean;
    try {
      acquired = await deps.cache.tryAcquireLock(service, lockToken, deps.timings.lockTtlMs);
    } catch {
      deps.logger.event('discovery.cache_unavailable', { service, stage: 'lock' });
      return degraded(service, 'cache-unavailable', null);
    }
    if (acquired) {
      try {
        return await probeAndStore(deps, service, token, lockToken);
      } catch {
        deps.logger.event('discovery.cache_unavailable', { service, stage: 'probe_store' });
        return degraded(service, 'cache-unavailable', null);
      }
    }

    if (deps.now() >= deadline) {
      // Invariant 2: a waiter NEVER probes as a fallback. One final read first, in case the
      // winner wrote just as the deadline expired — otherwise we report a false outage.
      deps.logger.event('discovery.lock_timeout', { service });
      const last = await readRecord(deps, service).catch(() => null);
      if (last !== null) return toState(service, last) ?? degraded(service, 'timeout', last);
      return degraded(service, 'timeout', null);
    }

    await sleep(backoff(attempt));
    const reread = await readRecord(deps, service).catch(() => null);
    if (reread !== null && isFresh(reread, deps.now(), deps.timings)) {
      return toState(service, reread) ?? degraded(service, 'network', reread);
    }
  }
}
```

- [ ] **Step 4: Run to verify it passes**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/single-flight.test.ts
```

Expected: PASS, 14 tests.

- [ ] **Step 5: Commit**

```bash
git add ts/packages/paigasus-discovery/src ts/packages/paigasus-discovery/tests
git commit -m "feat(ts): single-flight resolution with stale-while-revalidate and a fenced write (SMA-509)"
```

---

### Task 6: Config validation and the `createDiscovery` handle

**Files:**
- Create: `ts/packages/paigasus-discovery/src/config.ts`
- Create: `ts/packages/paigasus-discovery/src/server.ts`
- Test: `ts/packages/paigasus-discovery/tests/config.test.ts`
- Test: `ts/packages/paigasus-discovery/tests/discovery.test.ts`

**Interfaces:**
- Produces: `discoveryEnvShape`, `parseServiceMap(raw)`, `createDiscovery(deps): Discovery`, `type Discovery = { getServiceState, hasCapability }`.

- [ ] **Step 1: Write the failing config test**

`ts/packages/paigasus-discovery/tests/config.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { parseServiceMap } from '../src/config.js';

describe('parseServiceMap', () => {
  it('accepts a well-formed map', () => {
    const map = parseServiceMap('{"iam":"http://iam:8080","gateway":"https://gw.internal"}');
    expect(map).toEqual({ iam: 'http://iam:8080', gateway: 'https://gw.internal' });
  });

  it('accepts an empty map', () => {
    expect(parseServiceMap('{}')).toEqual({});
  });

  it('rejects an unknown service key', () => {
    // An operator typo would otherwise silently empty the console: the key never matches a
    // capability's first dot-segment, every service reads `absent`, and nothing renders.
    expect(() => parseServiceMap('{"iamm":"http://iam:8080"}')).toThrow(/iamm/);
  });

  it('rejects a key that is not lowercase', () => {
    expect(() => parseServiceMap('{"IAM":"http://iam:8080"}')).toThrow(/IAM/);
  });

  it.each(['ftp://iam:8080', 'file:///etc/passwd', 'not-a-url'])('rejects the scheme in %s', (url) => {
    expect(() => parseServiceMap(JSON.stringify({ iam: url }))).toThrow();
  });

  it('rejects userinfo in the URL', () => {
    expect(() => parseServiceMap('{"iam":"http://user:pass@iam:8080"}')).toThrow();
  });

  it('rejects a query or fragment', () => {
    expect(() => parseServiceMap('{"iam":"http://iam:8080/?a=1"}')).toThrow();
    expect(() => parseServiceMap('{"iam":"http://iam:8080/#x"}')).toThrow();
  });

  it('strips a trailing slash so the probe path never doubles it', () => {
    expect(parseServiceMap('{"iam":"http://iam:8080/"}')).toEqual({ iam: 'http://iam:8080' });
  });

  it('rejects a non-object', () => {
    expect(() => parseServiceMap('[]')).toThrow();
    expect(() => parseServiceMap('"iam"')).toThrow();
  });

  it('rejects unparseable JSON', () => {
    expect(() => parseServiceMap('{oops')).toThrow();
  });

  it('produces a null-prototype object', () => {
    // Same prototype-pollution rule @paigasus/next-config applies to PAIGASUS_ZONES.
    expect(Object.getPrototypeOf(parseServiceMap('{"iam":"http://iam:8080"}'))).toBeNull();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/config.test.ts
```

Expected: FAIL — cannot resolve `../src/config.js`.

- [ ] **Step 3: Write `src/config.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
import { z } from 'zod';
import { SERVICE_SLUGS } from './core/state.js';
import { DEFAULT_TIMINGS } from './core/record.js';

// PAIGASUS_ZONES' transform is NOT reusable here: `zoneMapFromJson` validates each value with
// `canonicalBasePath`, which is a PATH validator, not a URL validator.

const SLUG_RE = /^[a-z][a-z0-9]*$/;

/**
 * Parse and validate the service address map.
 *
 * Keys are checked against the closed set derived from the capability registry. An unknown key
 * FAILS CONSTRUCTION rather than being ignored: ignoring it produces a silently empty console,
 * which the design calls the worse failure because a wrongly-hidden item is invisible.
 */
export function parseServiceMap(raw: string): Record<string, string> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error('PAIGASUS_SERVICES must be a JSON object mapping service name to base URL');
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    throw new Error('PAIGASUS_SERVICES must be a JSON object mapping service name to base URL');
  }

  const out: Record<string, string> = Object.create(null) as Record<string, string>;
  for (const [key, value] of Object.entries(parsed)) {
    if (!SLUG_RE.test(key)) {
      throw new Error(`PAIGASUS_SERVICES: "${key}" is not a valid service name (expected ^[a-z][a-z0-9]*$)`);
    }
    if (!SERVICE_SLUGS.includes(key)) {
      throw new Error(
        `PAIGASUS_SERVICES: "${key}" is not a known service (expected one of ${SERVICE_SLUGS.join(', ')})`,
      );
    }
    if (typeof value !== 'string') {
      throw new Error(`PAIGASUS_SERVICES: the address for "${key}" must be a string`);
    }
    out[key] = canonicalBaseUrl(key, value);
  }
  return out;
}

function canonicalBaseUrl(key: string, value: string): string {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error(`PAIGASUS_SERVICES: the address for "${key}" is not an absolute URL`);
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new Error(`PAIGASUS_SERVICES: the address for "${key}" must use http: or https:`);
  }
  if (url.username !== '' || url.password !== '') {
    throw new Error(`PAIGASUS_SERVICES: the address for "${key}" must not carry credentials`);
  }
  if (url.search !== '' || url.hash !== '') {
    throw new Error(`PAIGASUS_SERVICES: the address for "${key}" must not carry a query or fragment`);
  }
  const path = url.pathname.replace(/\/+$/, '');
  return `${url.origin}${path}`;
}

const positiveInt = (fallback: number) =>
  z
    .string()
    .optional()
    .transform((v) => (v === undefined || v === '' ? fallback : Number(v)))
    .pipe(z.number().int().positive());

/**
 * The variables this package owns, composed into an app's `defineRuntimeConfig()` call.
 *
 * NOTE: `describeIssues` in @paigasus/next-config renders `issue.message` only for the keys it
 * owns itself, so a failure here surfaces as `PAIGASUS_SERVICES: custom`. The README therefore
 * carries the expected form of every variable.
 */
export const discoveryEnvShape = {
  PAIGASUS_SERVICES: z.string().transform((raw, ctx) => {
    try {
      return parseServiceMap(raw);
    } catch (err) {
      ctx.addIssue({ code: 'custom', message: err instanceof Error ? err.message : 'invalid' });
      return z.NEVER;
    }
  }),
  PAIGASUS_DISCOVERY_NEGATIVE_MS: positiveInt(DEFAULT_TIMINGS.negativeMs),
  PAIGASUS_DISCOVERY_FRESH_MS: positiveInt(DEFAULT_TIMINGS.freshMs),
  PAIGASUS_DISCOVERY_STALE_MS: positiveInt(DEFAULT_TIMINGS.staleMs),
  PAIGASUS_DISCOVERY_PROBE_TIMEOUT_MS: positiveInt(DEFAULT_TIMINGS.probeTimeoutMs),
  PAIGASUS_DISCOVERY_LOCK_WAIT_MS: positiveInt(DEFAULT_TIMINGS.lockWaitMs),
  PAIGASUS_DISCOVERY_LOCK_TTL_MS: positiveInt(DEFAULT_TIMINGS.lockTtlMs),
};
```

- [ ] **Step 4: Write the failing discovery-handle test**

`ts/packages/paigasus-discovery/tests/discovery.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from 'vitest';
import { createMemoryDescriptorCache } from '../src/adapters/memory-cache.js';
import { createDiscovery } from '../src/server.js';
import type { ProbeOutcome } from '../src/probe.js';

const descriptor = { service: 'iam', version: '1.0.0', capabilities: ['iam.audit'] };

function make(over: Parameters<typeof createDiscovery>[0] extends infer _ ? Record<string, unknown> : never = {}) {
  return createDiscovery({
    services: { iam: 'http://iam:8080' },
    cache: createMemoryDescriptorCache(),
    probe: async (): Promise<ProbeOutcome> => ({ ok: true, descriptor }),
    ...over,
  });
}

describe('createDiscovery', () => {
  it('reports absent for a service missing from the map, without touching the cache', async () => {
    const cache = createMemoryDescriptorCache();
    const get = vi.spyOn(cache, 'get');
    const d = make({ cache });
    expect(await d.getServiceState('gateway', 'tok')).toEqual({ state: 'absent', service: 'gateway' });
    expect(get).not.toHaveBeenCalled();
  });

  it('reports available for a configured, answering service', async () => {
    expect(await make().getServiceState('iam', 'tok')).toMatchObject({ state: 'available' });
  });

  it('memoizes within one handle so N calls cost one probe', async () => {
    const probe = vi.fn(async (): Promise<ProbeOutcome> => ({ ok: true, descriptor }));
    const d = make({ probe });
    await Promise.all([
      d.getServiceState('iam', 'tok'),
      d.getServiceState('iam', 'tok'),
      d.getServiceState('iam', 'tok'),
    ]);
    expect(probe).toHaveBeenCalledTimes(1);
  });

  it('rejects a timing order that would make a cold loser report a false outage', () => {
    // A loser whose deadline expires before the winner can finish probing AND writing reports a
    // healthy service as down on the first render after every deploy.
    expect(() => make({ timings: { probeTimeoutMs: 3_000, lockWaitMs: 2_000 } })).toThrow(/lockWaitMs/);
    expect(() => make({ timings: { lockWaitMs: 6_000, lockTtlMs: 5_000 } })).toThrow(/lockTtlMs/);
    expect(() => make({ timings: { negativeMs: 90_000, freshMs: 60_000 } })).toThrow(/freshMs/);
    expect(() => make({ timings: { freshMs: 700_000, staleMs: 600_000 } })).toThrow(/staleMs/);
  });
});

describe('hasCapability', () => {
  it('is true only when available and the key is present', async () => {
    const d = make();
    expect(await d.hasCapability('iam.audit', 'tok')).toBe(true);
    expect(await d.hasCapability('iam.apikeys', 'tok')).toBe(false);
  });

  it('is false for a degraded service even when the stale descriptor has the key', async () => {
    // The feature must not be invoked. <Capability> still RENDERS it disabled — the two
    // deliberately disagree, and the README says so.
    const d = make({ probe: async (): Promise<ProbeOutcome> => ({ ok: false, reason: 'network' }) });
    expect(await d.hasCapability('iam.audit', 'tok')).toBe(false);
  });

  it('is false for an absent service', async () => {
    expect(await make().hasCapability('gateway.chat.stream', 'tok')).toBe(false);
  });

  it('is false for an unknown key, without throwing', async () => {
    // Decision 6: unknown key -> ignore.
    expect(await make().hasCapability('iam.future' as never, 'tok')).toBe(false);
  });
});
```

- [ ] **Step 5: Run to verify it fails**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/discovery.test.ts
```

Expected: FAIL — cannot resolve `../src/server.js`.

- [ ] **Step 6: Write `src/server.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
import 'server-only';

import { DEFAULT_TIMINGS, type Timings } from './core/record.js';
import { serviceOf } from './core/state.js';
import { resolveService, type ResolveDeps } from './core/single-flight.js';
import { probeService, type ProbeOutcome } from './probe.js';
import { noopLogger } from './adapters/noop-logger.js';
import type { DescriptorCache } from './ports/cache.js';
import type { DiscoveryLogger } from './ports/logger.js';
import type { CapabilityKey, ServiceState } from './types.js';

export type { DescriptorCache } from './ports/cache.js';
export type { DiscoveryLogger, DiscoveryEventName, DiscoveryEventFields } from './ports/logger.js';
export { createMemoryDescriptorCache } from './adapters/memory-cache.js';
// NOTE: `export { createRedisDescriptorCache } from './adapters/redis-cache.js';` is added by
// Task 7, which creates that file. Do not add it here — the module would not resolve.
export { noopLogger } from './adapters/noop-logger.js';
export { discoveryEnvShape, parseServiceMap } from './config.js';
export { DEFAULT_TIMINGS } from './core/record.js';
export type { Timings } from './core/record.js';
export type { ServiceState, ServiceDescriptor, DegradedReason, CapabilityKey } from './types.js';

const MAX_DESCRIPTOR_BYTES = 64 * 1024;

export type CreateDiscoveryDeps = {
  readonly services: Readonly<Record<string, string>>;
  readonly cache: DescriptorCache;
  readonly logger?: DiscoveryLogger;
  readonly fetch?: typeof globalThis.fetch;
  readonly waitUntil?: (p: Promise<unknown>) => void;
  readonly timings?: Partial<Timings>;
  readonly now?: () => number;
  /** Test seam. Production always uses `probeService`. */
  readonly probe?: (service: string, token: string) => Promise<ProbeOutcome>;
};

export type Discovery = {
  getServiceState(service: string, token: string): Promise<ServiceState>;
  hasCapability(key: CapabilityKey, token: string): Promise<boolean>;
};

function resolveTimings(over: Partial<Timings> | undefined): Timings {
  const t: Timings = { ...DEFAULT_TIMINGS, ...over };
  // The probe must fit inside the wait, and the wait inside the lock's lifetime. A loser whose
  // deadline expires before the winner can probe AND write reports a false outage on the first
  // render after every deploy.
  if (!(t.probeTimeoutMs < t.lockWaitMs)) {
    throw new Error(`discovery: probeTimeoutMs (${t.probeTimeoutMs}) must be < lockWaitMs (${t.lockWaitMs})`);
  }
  if (!(t.lockWaitMs < t.lockTtlMs)) {
    throw new Error(`discovery: lockWaitMs (${t.lockWaitMs}) must be < lockTtlMs (${t.lockTtlMs})`);
  }
  if (!(t.negativeMs < t.freshMs)) {
    throw new Error(`discovery: negativeMs (${t.negativeMs}) must be < freshMs (${t.freshMs})`);
  }
  if (!(t.freshMs < t.staleMs)) {
    throw new Error(`discovery: freshMs (${t.freshMs}) must be < staleMs (${t.staleMs})`);
  }
  return t;
}

/**
 * Build a request-scoped discovery handle.
 *
 * There is deliberately NO module-level singleton: the composition root builds one handle, which
 * is what makes this testable without module resets and matches @paigasus/auth's
 * createAuthRuntime().
 *
 * `getServiceState` memoizes per handle. Without it a nav with eight <Capability> items over two
 * services costs eight cache reads per render, because React renders server components in tree
 * order and each sibling would resolve serially — which is also why the "one lock wait"
 * worst-case latency bound is stated conditionally on concurrent resolution.
 */
export function createDiscovery(deps: CreateDiscoveryDeps): Discovery {
  const timings = resolveTimings(deps.timings);
  const logger = deps.logger ?? noopLogger;
  const fetchImpl = deps.fetch ?? globalThis.fetch;
  const inflight = new Map<string, Promise<ServiceState>>();

  const probe =
    deps.probe ??
    ((service: string, token: string): Promise<ProbeOutcome> =>
      probeService({
        baseUrl: deps.services[service] ?? '',
        token,
        timeoutMs: timings.probeTimeoutMs,
        maxBytes: MAX_DESCRIPTOR_BYTES,
        fetch: fetchImpl,
      }));

  const resolveDeps: ResolveDeps = {
    cache: deps.cache,
    logger,
    timings,
    now: deps.now ?? Date.now,
    probe,
    ...(deps.waitUntil === undefined ? {} : { waitUntil: deps.waitUntil }),
  };

  function getServiceState(service: string, token: string): Promise<ServiceState> {
    if (deps.services[service] === undefined) {
      // ABSENT is decided from config alone. No cache read, no probe.
      return Promise.resolve({ state: 'absent', service });
    }
    const existing = inflight.get(service);
    if (existing !== undefined) return existing;
    const pending = resolveService(resolveDeps, service, token);
    inflight.set(service, pending);
    return pending;
  }

  async function hasCapability(key: CapabilityKey, token: string): Promise<boolean> {
    const state = await getServiceState(serviceOf(key), token);
    // `degraded` is FALSE: the feature must not be invoked. <Capability> still renders it
    // disabled rather than hidden — the two deliberately disagree.
    return state.state === 'available' && state.capabilities.includes(key);
  }

  return { getServiceState, hasCapability };
}
```

- [ ] **Step 7: Run both tests to verify they pass**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/config.test.ts tests/discovery.test.ts
```

Expected: PASS, 15 tests.

- [ ] **Step 8: Commit**

```bash
git add ts/packages/paigasus-discovery/src ts/packages/paigasus-discovery/tests
git commit -m "feat(ts): discovery env shape, URL validation, and the createDiscovery handle (SMA-509)"
```

---

### Task 7: The Redis adapter

**Files:**
- Create/complete: `ts/packages/paigasus-discovery/src/adapters/redis-cache.ts`
- Test: `ts/packages/paigasus-discovery/tests/redis-cache-options.test.ts`

**Interfaces:**
- Produces: `createRedisDescriptorCache(client, options?)` where `options` is `{ keyPrefix?: string }`.

- [ ] **Step 1: Write the failing precondition test**

`ts/packages/paigasus-discovery/tests/redis-cache-options.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { createRedisDescriptorCache } from '../src/adapters/redis-cache.js';

function fakeClient(over: Record<string, unknown> = {}): never {
  return {
    options: { disableOfflineQueue: true },
    listenerCount: () => 1,
    ...over,
  } as never;
}

describe('createRedisDescriptorCache preconditions', () => {
  it('accepts a correctly configured client', () => {
    expect(() => createRedisDescriptorCache(fakeClient())).not.toThrow();
  });

  it('refuses a client without disableOfflineQueue', () => {
    // Without it a Redis outage becomes HUNG REQUESTS rather than fast failures — node-redis
    // queues commands while disconnected. @paigasus/auth records this as load-bearing.
    expect(() => createRedisDescriptorCache(fakeClient({ options: { disableOfflineQueue: false } }))).toThrow(
      /disableOfflineQueue/,
    );
  });

  it('refuses a client with no error listener', () => {
    // node-redis emits 'error' on an EventEmitter; with no listener Node crashes the process.
    expect(() => createRedisDescriptorCache(fakeClient({ listenerCount: () => 0 }))).toThrow(/error listener/);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/redis-cache-options.test.ts
```

Expected: FAIL.

- [ ] **Step 3: Write `src/adapters/redis-cache.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
import type { RedisClientType } from 'redis';
import { parseRecord, type CacheRecord } from '../core/record.js';
import type { DescriptorCache } from '../ports/cache.js';

/**
 * Compare-and-set on `rev`. Invariant 5: a lock's compare-and-delete protects the LOCK, never the
 * WRITE. ARGV[3] is the empty string to mean "only if absent".
 *
 * `tonumber` on both sides, not a string compare: cjson can render an integer as a float, the
 * same trap @paigasus/auth's SET_CAS records.
 */
const SET_CAS = `
local cur = redis.call('GET', KEYS[1])
if ARGV[3] == '' then
  if cur then return 0 end
else
  if not cur then return 0 end
  local ok, parsed = pcall(cjson.decode, cur)
  if not ok or tonumber(parsed.rev) ~= tonumber(ARGV[3]) then return 0 end
end
redis.call('SET', KEYS[1], ARGV[1], 'PX', tonumber(ARGV[2]))
return 1
`;

/** Compare-and-delete, so a holder whose TTL expired cannot release the next holder's lock. */
const UNLOCK = `
if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('DEL', KEYS[1]) end
return 0
`;

export type RedisDescriptorCacheOptions = {
  /** '' in production: all zones share ONE store. Tests use a random prefix per test. */
  readonly keyPrefix?: string;
};

type ClientLike = RedisClientType & {
  options?: { disableOfflineQueue?: boolean };
  listenerCount(event: string): number;
};

/**
 * Wrap an ALREADY-CONNECTED node-redis client.
 *
 * The package never opens a connection: the composition root decides whether to share
 * @paigasus/auth's client or open a second one, and that decision is out of this issue's scope.
 *
 * The two preconditions below are ASSERTED rather than documented. A precondition nobody checks
 * is a precondition nobody keeps, and both failures are silent until production.
 */
export function createRedisDescriptorCache(
  client: RedisClientType,
  options: RedisDescriptorCacheOptions = {},
): DescriptorCache {
  const c = client as ClientLike;
  if (c.options?.disableOfflineQueue !== true) {
    throw new Error(
      'createRedisDescriptorCache: the client must be created with `disableOfflineQueue: true`, ' +
        'or a Redis outage becomes hung page renders instead of fast failures',
    );
  }
  if (c.listenerCount('error') === 0) {
    throw new Error(
      'createRedisDescriptorCache: the client must have an `error` listener, or node-redis ' +
        'crashes the process on a connection error. It must never log the raw error, which embeds the DSN',
    );
  }

  const prefix = options.keyPrefix ?? '';
  const recKey = (service: string): string => `${prefix}pgs:svcinfo:${service}`;
  const lockKey = (service: string): string => `${prefix}pgs:svcinfo:lock:${service}`;

  return {
    async get(service) {
      const raw = await client.get(recKey(service));
      if (raw === null) return null;
      const parsed = parseRecord(raw);
      if (parsed === null) {
        // A foreign schema version or a corrupt value. Delete it: during a rolling upgrade two
        // console builds write this key, and serving an unreadable record would poison the whole
        // fleet for the hard TTL.
        await client.del(recKey(service));
        return null;
      }
      return parsed;
    },

    async set(service, rec: CacheRecord, ttlMs, expectedRev) {
      const result = await client.eval(SET_CAS, {
        keys: [recKey(service)],
        arguments: [JSON.stringify(rec), String(ttlMs), expectedRev === null ? '' : String(expectedRev)],
      });
      return result === 1;
    },

    async delete(service) {
      await client.del(recKey(service));
    },

    async tryAcquireLock(service, token, ttlMs) {
      // A single atomic primitive; no script needed for acquisition.
      const res = await client.set(lockKey(service), token, {
        condition: 'NX',
        expiration: { type: 'PX', value: ttlMs },
      });
      return res === 'OK';
    },

    async releaseLock(service, token) {
      await client.eval(UNLOCK, { keys: [lockKey(service)], arguments: [token] });
    },

    close() {
      // The client is INJECTED, so its lifetime belongs to whoever created it. Closing it here
      // would tear down @paigasus/auth's session store when the two share one connection.
      return Promise.resolve();
    },

    async writeRawForTest(service, raw) {
      await client.set(recKey(service), raw, { expiration: { type: 'PX', value: 60_000 } });
    },
  };
}
```

- [ ] **Step 4: Run to verify it passes**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/redis-cache-options.test.ts
```

Expected: PASS, 3 tests.

- [ ] **Step 5: Add the re-export Task 6 deferred**

In `ts/packages/paigasus-discovery/src/server.ts`, replace the two-line NOTE comment left by Task 6 with the real re-export, directly below the `createMemoryDescriptorCache` line:

```ts
export { createRedisDescriptorCache } from './adapters/redis-cache.js';
```

- [ ] **Step 6: Verify the package still type-checks and the whole suite passes**

```bash
pnpm -C ts/packages/paigasus-discovery exec tsc -p tsconfig.json --noEmit
pnpm -C ts/packages/paigasus-discovery exec vitest run
```

Expected: no type errors; every test to date passes.

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-discovery/src ts/packages/paigasus-discovery/tests
git commit -m "feat(ts): the Redis DescriptorCache adapter with asserted client preconditions (SMA-509)"
```

---

### Task 8: `<Capability>` and the disabled wrapper

Carries AC1. The wrapper is a **client** component because an async server component cannot attach the handlers that block activation.

**Files:**
- Create: `ts/packages/paigasus-discovery/src/disabled.tsx`
- Create: `ts/packages/paigasus-discovery/src/react.tsx`
- Test: `ts/packages/paigasus-discovery/tests/capability.test.tsx`

**Interfaces:**
- Consumes: `Discovery` from `src/server.ts`, `ServiceState` from `src/types.ts`.
- Produces: `<Capability discovery need token children degraded? />`, `<CapabilityDisabled reason children />`.

- [ ] **Step 1: Write the failing test**

`ts/packages/paigasus-discovery/tests/capability.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Capability } from '../src/react.js';
import { createMemoryDescriptorCache } from '../src/adapters/memory-cache.js';
import { createDiscovery } from '../src/server.js';
import type { ProbeOutcome } from '../src/probe.js';

const descriptor = { service: 'iam', version: '1.0.0', capabilities: ['iam.audit'] };

function discoveryWith(probe: () => Promise<ProbeOutcome>, services = { iam: 'http://iam:8080' }) {
  return createDiscovery({ services, cache: createMemoryDescriptorCache(), probe });
}

/**
 * <Capability> is an ASYNC SERVER COMPONENT. React's client renderer cannot render one, so the
 * element it returns is awaited and THAT is rendered. Calling render() on the component itself
 * is the mistake this helper exists to prevent.
 */
async function renderCapability(element: Promise<React.ReactElement | null>) {
  const resolved = await element;
  return render(resolved);
}

describe('AC1: three states', () => {
  it('absent: renders nothing at all', async () => {
    const discovery = discoveryWith(async () => ({ ok: true, descriptor }), {});
    const { container } = await renderCapability(
      Capability({ discovery, need: 'iam.audit', token: 'tok', children: <a href="/audit">Audit</a> }),
    );
    expect(container).toBeEmptyDOMElement();
  });

  it('available with the key: renders children untouched', async () => {
    const discovery = discoveryWith(async () => ({ ok: true, descriptor }));
    await renderCapability(
      Capability({ discovery, need: 'iam.audit', token: 'tok', children: <a href="/audit">Audit</a> }),
    );
    const link = screen.getByRole('link', { name: 'Audit' });
    expect(link).toBeInTheDocument();
    expect(link.closest('[aria-disabled="true"]')).toBeNull();
  });

  it('available without the key: renders nothing', async () => {
    // The service ANSWERED and said it lacks the feature. There is no outage to report, so
    // hiding is correct here and disabling is not.
    const discovery = discoveryWith(async () => ({ ok: true, descriptor }));
    const { container } = await renderCapability(
      Capability({ discovery, need: 'iam.apikeys', token: 'tok', children: <a href="/keys">Keys</a> }),
    );
    expect(container).toBeEmptyDOMElement();
  });
});

describe('AC1: degraded is rendered disabled, never hidden', () => {
  async function renderDegraded(cached: boolean) {
    const cache = createMemoryDescriptorCache();
    const discovery = createDiscovery({
      services: { iam: 'http://iam:8080' },
      cache,
      probe: async () => ({ ok: false, reason: 'network' }),
    });
    if (cached) {
      const warm = createDiscovery({
        services: { iam: 'http://iam:8080' },
        cache,
        probe: async () => ({ ok: true, descriptor }),
      });
      await warm.getServiceState('iam', 'tok');
    }
    return renderCapability(
      Capability({ discovery, need: 'iam.audit', token: 'tok', children: <a href="/audit">Audit</a> }),
    );
  }

  it.each([true, false])('renders the item, disabled with a reason (cached descriptor: %s)', async (cached) => {
    await renderDegraded(cached);
    const link = screen.getByRole('link', { name: 'Audit' });

    // NEVER HIDDEN: still present, and still in the accessibility tree.
    expect(link).toBeInTheDocument();

    const wrapper = link.closest('[data-capability-state]');
    expect(wrapper).not.toBeNull();
    expect(wrapper).toHaveAttribute('data-capability-state', 'degraded');
    expect(wrapper).toHaveAttribute('aria-disabled', 'true');

    // NOT `inert`: inert removes the node from the accessibility tree, which hides the item from
    // a screen reader and breaks "never hidden" for exactly the users least able to recover.
    expect(wrapper).not.toHaveAttribute('inert');

    // The reason is reachable, not merely present as a class.
    const describedBy = wrapper?.getAttribute('aria-describedby');
    expect(describedBy).toBeTruthy();
    expect(document.getElementById(describedBy as string)?.textContent).toMatch(/not answering|unreachable/i);
  });

  it('blocks activation — the assertion aria-disabled alone would let through', async () => {
    // THE REGRESSION THIS EXISTS FOR. `tabindex` is NOT inherited: putting tabindex="-1" on the
    // wrapper leaves a nested <a> fully keyboard-activatable while aria-disabled makes the
    // attribute assertions above pass. A green test over a broken control.
    const onNavigate = vi.fn();
    const cache = createMemoryDescriptorCache();
    const discovery = createDiscovery({
      services: { iam: 'http://iam:8080' },
      cache,
      probe: async () => ({ ok: false, reason: 'network' }),
    });
    await renderCapability(
      Capability({
        discovery,
        need: 'iam.audit',
        token: 'tok',
        children: (
          <a href="/audit" onClick={onNavigate}>
            Audit
          </a>
        ),
      }),
    );
    const link = screen.getByRole('link', { name: 'Audit' });
    await userEvent.click(link);
    expect(onNavigate).not.toHaveBeenCalled();

    link.focus();
    await userEvent.keyboard('{Enter}');
    expect(onNavigate).not.toHaveBeenCalled();
  });

  it('gives each instance a unique description id', async () => {
    // A hardcoded id emits duplicates when two <Capability> elements appear on one page.
    const discovery = discoveryWith(async () => ({ ok: false, reason: 'network' }));
    const a = await Capability({ discovery, need: 'iam.audit', token: 'tok', children: <span>A</span> });
    const b = await Capability({ discovery, need: 'iam.audit', token: 'tok', children: <span>B</span> });
    const { container } = render(
      <>
        {a}
        {b}
      </>,
    );
    const ids = [...container.querySelectorAll('[aria-describedby]')].map((e) =>
      e.getAttribute('aria-describedby'),
    );
    expect(ids).toHaveLength(2);
    expect(new Set(ids).size).toBe(2);
  });

  it('uses the degraded render prop when given one', async () => {
    const discovery = discoveryWith(async () => ({ ok: false, reason: 'timeout' }));
    await renderCapability(
      Capability({
        discovery,
        need: 'iam.audit',
        token: 'tok',
        children: <a href="/audit">Audit</a>,
        degraded: (reason) => <span data-testid="custom">{reason}</span>,
      }),
    );
    expect(screen.getByTestId('custom')).toHaveTextContent('timeout');
    expect(screen.queryByRole('link')).toBeNull();
  });

  it('ships no Tailwind utility classes', async () => {
    // A package shipping utility classes needs an `@source` line in EVERY consumer; forgetting it
    // drops them silently and ONLY in a production build (the SMA-503 failure class). The
    // functional bits are inline styles; everything cosmetic is a data attribute.
    const discovery = discoveryWith(async () => ({ ok: false, reason: 'network' }));
    const { container } = await renderCapability(
      Capability({ discovery, need: 'iam.audit', token: 'tok', children: <a href="/audit">Audit</a> }),
    );
    for (const el of container.querySelectorAll('*')) {
      expect(el.className, el.outerHTML).toBe('');
    }
  });
});
```

- [ ] **Step 2: Run to verify it fails**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/capability.test.tsx
```

Expected: FAIL — cannot resolve `../src/react.js`.

- [ ] **Step 3: Write `src/disabled.tsx`**

```tsx
'use client';
// SPDX-License-Identifier: Apache-2.0
//
// A CLIENT component, and it has to be. <Capability> is an async server component and cannot
// attach event handlers, but blocking activation requires them: `tabindex` is NOT inherited, so
// putting tabIndex={-1} on a wrapper leaves a nested <a> or <button> fully keyboard-activatable
// while `aria-disabled` makes every attribute assertion pass. Capture-phase handlers are what
// actually disable the subtree.
//
// It carries NO Tailwind utility classes. Tailwind v4's scan root is the working directory and
// Moon runs `next build` from the app's own directory, so a package shipping utility classes needs
// an `@source` line in every consumer — and forgetting it drops the classes silently, ONLY in a
// production build. Everything cosmetic is exposed as a data attribute for the consumer to style.

import { useId, type KeyboardEvent, type MouseEvent, type ReactElement, type ReactNode } from 'react';
import type { DegradedReason } from './types.js';

const REASON_TEXT: Readonly<Record<DegradedReason, string>> = {
  timeout: 'not answering (timed out)',
  network: 'not answering (unreachable)',
  unauthorized: 'not answering (your session was rejected)',
  'not-implemented': 'not answering (this build does not provide it)',
  'bad-response': 'not answering (unrecognised response)',
  'server-error': 'not answering (server error)',
  'cache-unavailable': 'not answering (discovery cache unavailable)',
};

export function reasonText(service: string, reason: DegradedReason): string {
  return `${service} is ${REASON_TEXT[reason]}`;
}

export type CapabilityDisabledProps = {
  readonly service: string;
  readonly reason: DegradedReason;
  readonly children: ReactNode;
};

export function CapabilityDisabled({ service, reason, children }: CapabilityDisabledProps): ReactElement {
  // useId, never a hardcoded id: two <Capability> elements on one page would otherwise emit
  // duplicate ids and break the aria-describedby association for both.
  const describedBy = useId();

  const block = (event: MouseEvent | KeyboardEvent): void => {
    event.preventDefault();
    event.stopPropagation();
  };

  return (
    <span
      data-capability-state="degraded"
      data-capability-reason={reason}
      aria-disabled="true"
      aria-describedby={describedBy}
      style={{ pointerEvents: 'none' }}
      onClickCapture={block}
      onKeyDownCapture={(event) => {
        if (event.key === 'Enter' || event.key === ' ') block(event);
      }}
    >
      {children}
      <span
        id={describedBy}
        style={{
          position: 'absolute',
          width: 1,
          height: 1,
          padding: 0,
          margin: -1,
          overflow: 'hidden',
          clip: 'rect(0 0 0 0)',
          whiteSpace: 'nowrap',
          border: 0,
        }}
      >
        {reasonText(service, reason)}
      </span>
    </span>
  );
}
```

- [ ] **Step 4: Write `src/react.tsx`**

```tsx
// SPDX-License-Identifier: Apache-2.0
import 'server-only';

import type { ReactElement, ReactNode } from 'react';
import { CapabilityDisabled } from './disabled.js';
import type { Discovery } from './server.js';
import type { CapabilityKey, DegradedReason } from './types.js';

export { CapabilityDisabled } from './disabled.js';

export type CapabilityProps = {
  readonly discovery: Discovery;
  readonly need: CapabilityKey;
  readonly token: string;
  readonly children: ReactNode;
  /** Replace the default disabled wrapper entirely. SMA-510's nav is the expected first user. */
  readonly degraded?: (reason: DegradedReason) => ReactNode;
};

/**
 * Gate feature UI on a service capability.
 *
 * COSMETIC ONLY, exactly like `can()`. The server remains authoritative and must handle an
 * unimplemented call gracefully; a client that treated this as a security boundary would be wrong.
 *
 * It branches on the FULL state, never on `hasCapability`:
 *
 *   absent                          -> nothing
 *   available, key present          -> children
 *   available, key absent           -> nothing
 *   degraded, descriptor has key    -> children, disabled with a reason
 *   degraded, no descriptor at all  -> children, disabled with a reason
 *
 * That last row is a DECISION, not a derivation: we do not know whether a never-reached service
 * has the capability. Disabling is chosen over hiding because hiding a deployed-but-down service
 * turns an outage into an apparent configuration change, and a wrongly-shown disabled item is
 * recoverable while a wrongly-hidden one is invisible.
 *
 * The `available, key absent` row hides rather than disables, and the asymmetry is deliberate:
 * there the service answered and told us it lacks the feature, so there is no outage to report.
 */
export async function Capability(props: CapabilityProps): Promise<ReactElement | null> {
  const { discovery, need, token, children, degraded } = props;
  const service = need.slice(0, need.indexOf('.'));
  const state = await discovery.getServiceState(service, token);

  if (state.state === 'absent') return null;
  if (state.state === 'available') {
    return state.capabilities.includes(need) ? <>{children}</> : null;
  }

  if (degraded !== undefined) return <>{degraded(state.reason)}</>;
  return (
    <CapabilityDisabled service={state.service} reason={state.reason}>
      {children}
    </CapabilityDisabled>
  );
}
```

- [ ] **Step 5: Run to verify it passes**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/capability.test.tsx
```

Expected: PASS, 10 tests.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-discovery/src ts/packages/paigasus-discovery/tests
git commit -m "feat(ts): <Capability> with an accessible, activation-blocking disabled state (SMA-509)"
```

---

### Task 9: Structure tests, the README, and AC4

**Files:**
- Create: `ts/packages/paigasus-discovery/README.md`
- Test: `ts/packages/paigasus-discovery/tests/structure/exports.test.ts`
- Test: `ts/packages/paigasus-discovery/tests/cosmetic.test.ts`

**Interfaces:** none new.

- [ ] **Step 1: Write the failing structure test**

`ts/packages/paigasus-discovery/tests/structure/exports.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const pkg = JSON.parse(
  readFileSync(fileURLToPath(new URL('../../package.json', import.meta.url)), 'utf8'),
) as { exports: Record<string, string>; dependencies: Record<string, string> };

describe('package boundary', () => {
  it('exposes exactly three subpaths and NO root export', () => {
    // A root export re-exporting the server surface would let a client component import it and
    // walk around the boundary — the reason @paigasus/auth omits one too.
    expect(Object.keys(pkg.exports).sort()).toEqual(['./react', './server', './types']);
    expect(pkg.exports['.']).toBeUndefined();
  });

  it('does not depend on @paigasus/sdk', () => {
    // Three reasons: the SDK's Presentation union cannot express a DegradedReason; the probe is
    // a bare fetch, so @connectrpc/* is pure cost; and `paigasus/boundaries/app-shell` bans the
    // SDK from app-shell, which a transitive edge through this package would violate.
    expect(Object.keys(pkg.dependencies)).not.toContain('@paigasus/sdk');
  });

  it('guards the server entries and deliberately leaves ./types unguarded', () => {
    const read = (rel: string): string =>
      readFileSync(fileURLToPath(new URL(`../../${rel}`, import.meta.url)), 'utf8');
    expect(read('src/server.ts')).toMatch(/^import 'server-only';/m);
    expect(read('src/react.tsx')).toMatch(/^import 'server-only';/m);
    // ./types is client-safe on purpose: a client component holds a resolved ServiceState.
    expect(read('src/types.ts')).not.toMatch(/server-only/);
  });

  it('never sets the react-server vitest condition', () => {
    // MEASURED in @paigasus/auth: react-server is also the condition `react`'s own exports map
    // switches on, and that build has no createContext, so setting it breaks react-dom/client
    // under @testing-library/react. The `server-only` alias is the fix instead.
    const config = readFileSync(fileURLToPath(new URL('../../vitest.config.ts', import.meta.url)), 'utf8');
    expect(config).not.toContain('react-server');
    expect(config).toContain("alias: { 'server-only'");
  });
});
```

- [ ] **Step 2: Write the failing AC4 test**

`ts/packages/paigasus-discovery/tests/cosmetic.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { createMemoryDescriptorCache } from '../src/adapters/memory-cache.js';
import { createDiscovery } from '../src/server.js';
import * as server from '../src/server.js';
import type { ProbeOutcome } from '../src/probe.js';

describe('AC4: capability gating is cosmetic', () => {
  it('exposes no enforcement hook — only data', () => {
    // If the package could BLOCK a call, "the server remains authoritative" would be false.
    // hasCapability returns a boolean and nothing here wraps, guards, or refuses a request.
    const names = Object.keys(server);
    for (const forbidden of ['enforce', 'require', 'guard', 'assert', 'authorize']) {
      expect(names.filter((n) => n.toLowerCase().includes(forbidden))).toEqual([]);
    }
  });

  it('a 404 from an older service degrades rather than throwing', () => {
    // ADR-0020 A2: a disabled capability's routes are unmounted (404), indistinguishable from a
    // build predating the feature. Neither may crash the console.
    const discovery = createDiscovery({
      services: { iam: 'http://iam:8080' },
      cache: createMemoryDescriptorCache(),
      probe: async (): Promise<ProbeOutcome> => ({ ok: false, reason: 'not-implemented' }),
    });
    return expect(discovery.getServiceState('iam', 'tok')).resolves.toMatchObject({
      state: 'degraded',
      reason: 'not-implemented',
    });
  });

  it('hasCapability being false never prevents the call being made', async () => {
    const discovery = createDiscovery({
      services: { iam: 'http://iam:8080' },
      cache: createMemoryDescriptorCache(),
      probe: async (): Promise<ProbeOutcome> => ({
        ok: true,
        descriptor: { service: 'iam', version: '1.0.0', capabilities: [] },
      }),
    });
    expect(await discovery.hasCapability('iam.audit', 'tok')).toBe(false);
    // Nothing above returns a blocker, a throw, or a wrapped client. The caller is free to
    // proceed and the server answers authoritatively.
  });

  it('the README states the cosmetic rule and the not-a-security-boundary rule', () => {
    const readme = readFileSync(fileURLToPath(new URL('../README.md', import.meta.url)), 'utf8');
    expect(readme).toMatch(/cosmetic/i);
    expect(readme).toMatch(/not a security boundary/i);
  });
});
```

- [ ] **Step 3: Run both to verify they fail**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/structure tests/cosmetic.test.ts
```

Expected: FAIL — README missing.

- [ ] **Step 4: Write the README**

`ts/packages/paigasus-discovery/README.md`:

````markdown
# @paigasus/discovery

Lazy, request-scoped service capability discovery for Paigasus console zones.

Addresses come from deployment configuration. Capabilities come from the service
itself, through the `paigasus.common.v1.ServiceInfo` descriptor. See ADR-0020.

## Capability gating is cosmetic

`hasCapability()` and `<Capability>` hide or disable affordances. They are
**cosmetic only**, exactly like `can()`.

**This is not a security boundary.** The server remains authoritative. Every
action is authorized server-side, and the console must render the resulting
error correctly. A capability list that said "yes" would not make a call
succeed, and one that says "no" does not prevent the call being made.

A service whose capability is disabled returns `404` (HTTP) or `UNIMPLEMENTED`
(gRPC), which is deliberately indistinguishable from a build predating the
feature — ADR-0020 A2. `@paigasus/sdk`'s `mapError` already maps both to a clean
`PaigasusError`; its own tests cover that path, and this package does not
duplicate them.

## The three states

| State | Condition | Render |
|---|---|---|
| absent | not in `PAIGASUS_SERVICES` | nothing |
| available | configured and answering | normal |
| degraded | configured but unreachable | **disabled with a reason, never hidden** |

Hiding a deployed-but-down service turns an outage into an apparent
configuration change and destroys the operator's signal.

Note that `hasCapability()` and `<Capability>` deliberately **disagree** for a
degraded service: `hasCapability()` returns `false`, because the feature must not
be invoked, while `<Capability>` still renders the item disabled rather than
hidden. Use `hasCapability()` for non-UI callers; use `<Capability>` to render.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `PAIGASUS_SERVICES` | *(required)* | JSON object, service name to base URL: `{"iam":"http://iam:8080"}`. Keys must be known service slugs; values must be absolute `http:`/`https:` URLs with no credentials, query or fragment. |
| `PAIGASUS_DISCOVERY_NEGATIVE_MS` | `10000` | how long a failed probe is honoured |
| `PAIGASUS_DISCOVERY_FRESH_MS` | `60000` | how long a successful probe is honoured |
| `PAIGASUS_DISCOVERY_STALE_MS` | `600000` | the Redis hard TTL; the stale window |
| `PAIGASUS_DISCOVERY_PROBE_TIMEOUT_MS` | `1500` | per-probe network deadline |
| `PAIGASUS_DISCOVERY_LOCK_WAIT_MS` | `2500` | how long a cold loser waits |
| `PAIGASUS_DISCOVERY_LOCK_TTL_MS` | `5000` | single-flight lock lifetime |

`NEGATIVE_MS < FRESH_MS < STALE_MS` and
`PROBE_TIMEOUT_MS < LOCK_WAIT_MS < LOCK_TTL_MS` are asserted at construction.

`@paigasus/next-config`'s `describeIssues` renders a full message only for keys
it owns itself, so a malformed value surfaces as `PAIGASUS_SERVICES: custom`.
The table above is the authoritative statement of each variable's form.

## Usage

```ts
import { createDiscovery, createRedisDescriptorCache } from '@paigasus/discovery/server';

const discovery = createDiscovery({
  services: config.PAIGASUS_SERVICES,
  cache: createRedisDescriptorCache(redisClient),
  waitUntil: after,          // from 'next/server'
});
```

Build **one handle per request**: `getServiceState` memoizes per handle, so N
`<Capability>` elements over one service cost one resolution.

The Redis client is **injected and already connected**. It must be created with
`disableOfflineQueue: true` and an `error` listener — both are asserted, because
without the first a Redis outage becomes hung page renders, and without the
second node-redis crashes the process. The error listener must never log the raw
error, which embeds the DSN.

### `waitUntil`

Stale-while-revalidate refreshes in the background. On the self-hosted container
deploy target the process survives the response and a floated promise completes,
so `waitUntil` is optional. On a platform that freezes the container at response
end it is **required**; without it the effective refresh interval degrades to
`PAIGASUS_DISCOVERY_STALE_MS`.

### Consuming from `@paigasus/app-shell`

Don't. `<Capability>` is an async server component and this package's `./react`
and `./server` entries are `server-only`. `paigasus/boundaries/app-shell` keeps
app-shell client-reachable. App-shell exports navigation *presentation* taking
resolved state as props; the **app** composes `<Capability>` around it.
````

- [ ] **Step 5: Run both to verify they pass**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run tests/structure tests/cosmetic.test.ts
```

Expected: PASS, 8 tests.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-discovery/README.md ts/packages/paigasus-discovery/tests
git commit -m "docs(ts): discovery README and the cosmetic-gating assertions (SMA-509)"
```

---

### Task 10: The real-Redis container tier (AC3 half two)

**Files:**
- Create: `ts/packages/paigasus-discovery/vitest.containers.config.ts`
- Create: `ts/packages/paigasus-discovery/tests/containers/support/redis.ts`
- Test: `ts/packages/paigasus-discovery/tests/containers/redis-cache.test.ts`
- Test: `ts/packages/paigasus-discovery/tests/containers/single-flight-multiprocess.test.ts`
- Create: `ts/packages/paigasus-discovery/tests/containers/support/worker.ts`

**Interfaces:** none new.

- [ ] **Step 1: Write the containers vitest config**

`ts/packages/paigasus-discovery/vitest.containers.config.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// A SEPARATE config, not a CLI path filter on the default one: vitest applies a positional path
// AFTER `exclude`, so `vitest run --config vitest.config.ts tests/containers/` matches zero files
// and passes vacuously. A suite that silently runs nothing is worse than a red.
//
// There is NO SKIP HATCH when Docker is unreachable. This suite fails loudly, the precedent
// @paigasus/auth set deliberately against paigasus-iam's silently-skipping Docker suites.
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

- [ ] **Step 2: Write the container support helper**

`ts/packages/paigasus-discovery/tests/containers/support/redis.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { GenericContainer, type StartedTestContainer } from 'testcontainers';
import { createClient, type RedisClientType } from 'redis';

export type RedisFixture = {
  readonly url: string;
  readonly container: StartedTestContainer;
};

export async function startRedis(): Promise<RedisFixture> {
  const container = await new GenericContainer('redis:8-alpine').withExposedPorts(6379).start();
  return { url: `redis://${container.getHost()}:${container.getMappedPort(6379)}`, container };
}

/**
 * A client carrying the options createRedisDescriptorCache asserts. Mirrors the production shape
 * in @paigasus/auth's createRedisSessionStore.
 */
export async function connect(url: string): Promise<RedisClientType> {
  const client: RedisClientType = createClient({ url, disableOfflineQueue: true });
  // Never log the raw error: node-redis embeds the DSN in its connection errors.
  client.on('error', () => undefined);
  await client.connect();
  return client;
}

/** A fresh prefix per test, so one container serves the whole file without cross-test leakage. */
export function randomPrefix(): string {
  return `d${Date.now().toString(36)}${Math.random().toString(36).slice(2)}:`;
}
```

- [ ] **Step 3: Write the adapter contract test against real Redis**

`ts/packages/paigasus-discovery/tests/containers/redis-cache.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { afterAll, beforeAll } from 'vitest';
import type { RedisClientType } from 'redis';
import { createRedisDescriptorCache } from '../../src/adapters/redis-cache.js';
import { runCacheContract } from '../cache-contract.js';
import { connect, randomPrefix, startRedis, type RedisFixture } from './support/redis.js';

let fixture: RedisFixture;
let client: RedisClientType;

beforeAll(async () => {
  fixture = await startRedis();
  client = await connect(fixture.url);
});

afterAll(async () => {
  await client?.quit();
  await fixture?.container.stop();
});

// The SAME contract the memory adapter runs. A semantic divergence — a lock that is not atomic,
// a fence that does not fence, a foreign schema version that is served rather than deleted —
// fails here rather than in production.
runCacheContract('redis', async () =>
  createRedisDescriptorCache(client, { keyPrefix: randomPrefix() }),
);
```

- [ ] **Step 4: Write the cross-process single-flight test**

`ts/packages/paigasus-discovery/tests/containers/support/worker.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Runs in a FORKED process. Two console zones are two processes, not two async callers sharing
// one event loop — an in-process test cannot distinguish a real distributed lock from the event
// loop merely serializing everything.
import { createRedisDescriptorCache } from '../../../src/adapters/redis-cache.js';
import { noopLogger } from '../../../src/adapters/noop-logger.js';
import { DEFAULT_TIMINGS } from '../../../src/core/record.js';
import { resolveService } from '../../../src/core/single-flight.js';
import { connect } from './redis.js';

async function main(): Promise<void> {
  const [url, prefix, probeMs] = process.argv.slice(2);
  const client = await connect(url as string);
  let probes = 0;
  const state = await resolveService(
    {
      cache: createRedisDescriptorCache(client, { keyPrefix: prefix as string }),
      logger: noopLogger,
      timings: { ...DEFAULT_TIMINGS, lockWaitMs: 4_000, probeTimeoutMs: 3_000, lockTtlMs: 8_000 },
      now: Date.now,
      probe: async () => {
        probes += 1;
        await new Promise((r) => setTimeout(r, Number(probeMs)));
        return { ok: true, descriptor: { service: 'iam', version: '1.0.0', capabilities: ['iam.audit'] } };
      },
    },
    'iam',
    'tok',
  );
  process.stdout.write(JSON.stringify({ probes, state: state.state }));
  await client.quit();
}

void main();
```

`ts/packages/paigasus-discovery/tests/containers/single-flight-multiprocess.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { execFile } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { randomPrefix, startRedis, type RedisFixture } from './support/redis.js';

const run = promisify(execFile);
const worker = fileURLToPath(new URL('./support/worker.ts', import.meta.url));

let fixture: RedisFixture;

beforeAll(async () => {
  fixture = await startRedis();
});

afterAll(async () => {
  await fixture?.container.stop();
});

describe('AC3: single-flight across PROCESSES', () => {
  it('four independent processes trigger exactly one probe', async () => {
    // This is the half a fake cannot prove: that SET NX PX and the compare-and-delete release are
    // genuinely atomic in Redis, rather than merely appearing so because one event loop
    // serialized every caller.
    const prefix = randomPrefix();
    const results = await Promise.all(
      Array.from({ length: 4 }, () =>
        run(process.execPath, ['--experimental-strip-types', worker, fixture.url, prefix, '600']),
      ),
    );
    const parsed = results.map((r) => JSON.parse(r.stdout) as { probes: number; state: string });
    expect(parsed.reduce((sum, p) => sum + p.probes, 0)).toBe(1);
    for (const p of parsed) expect(p.state).toBe('available');
  });
});
```

- [ ] **Step 5: Run the container suite**

```bash
pnpm -C ts/packages/paigasus-discovery exec vitest run --config vitest.containers.config.ts
```

Expected: PASS. If Docker is unreachable it must FAIL LOUDLY, never skip. If the worker fails to load TypeScript, replace `--experimental-strip-types` with the flag Node 24 accepts in this repo and re-run; do not weaken the assertion.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-discovery/vitest.containers.config.ts ts/packages/paigasus-discovery/tests/containers
git commit -m "test(ts): real-Redis contract and cross-process single-flight for discovery (SMA-509)"
```

---

### Task 11: Moon wiring, the ESLint boundary, and the affected-graph re-baseline

The gate work. Nothing here is optional; each item reds CI if skipped.

**Files:**
- Create: `ts/packages/paigasus-discovery/moon.yml`
- Modify: `ts/packages/paigasus-next-config/src/eslint.mjs`
- Modify: `ts/packages/paigasus-next-config/tests/boundaries.test.ts`
- Modify: `ci/affected-graph/run.sh`
- Modify: `.github/workflows/ci.yml` (comment only)

**Interfaces:** none new.

- [ ] **Step 1: Write the moon.yml**

`ts/packages/paigasus-discovery/moon.yml`:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-discovery-ts'
layer: 'library'
language: 'typescript'

# BOTH are required and neither implies the other. `dependsOn` is what `moon query projects
# --affected` follows and what orders the upstream's build; task `inputs` are the ONLY thing that
# confers affectedness on a DOWNSTREAM.
dependsOn: ['paigasus-proto-ts']

tasks:
  # APPEND, never `options.merge: replace`. The inherited definitions in
  # .moon/tasks/typescript-project.yml supply @group(sources), @group(tests), tsconfig.json,
  # package.json, /ts/tsconfig.base.json and /ts/pnpm-lock.yaml; replacing would silently drop all
  # of them — the defect SMA-503 fixed on paigasus-console-ts:build.
  build:
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
  typecheck:
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
  test:
    # vitest.config.ts matches neither src/**/* nor tests/**/*, so without this line an edit to the
    # `server-only` alias or the resolution conditions serves a cached PASS — and that alias is the
    # only thing keeping server-only's unconditional throw from failing the whole suite.
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
      - 'vitest.config.ts'
  test-e2e:
    # `script` (not `command`): Moon's `command` setting rejects shell syntax outright.
    # `set -euo pipefail` mirrors every other `script:` task here — Moon does not enable errexit
    # for script blocks on its own.
    script: |
      set -euo pipefail
      pnpm exec vitest run --config vitest.containers.config.ts
    inputs:
      - '@group(sources)'
      - '@group(tests)'
      - 'vitest.config.ts'
      - 'vitest.containers.config.ts'
      - 'package.json'
      - '/ts/pnpm-lock.yaml'
    options:
      # Real containers. A cached PASS would replay a green that tested nothing.
      cache: false
```

- [ ] **Step 2: Add the ESLint boundary block**

In `ts/packages/paigasus-next-config/src/eslint.mjs`, add a block immediately after the `paigasus/boundaries/app-shell` block:

```js
  {
    name: 'paigasus/boundaries/discovery',
    files: ['packages/paigasus-discovery/**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}'],
    rules: restrict([
      {
        group: ['next', 'next/*', 'next/**'],
        message:
          '@paigasus/discovery must not import from `next`. Next primitives are INJECTED — `waitUntil` takes `after` as a parameter — so the package tests without a Next runtime (§ 8.1).',
      },
      {
        group: ['@paigasus/sdk', '@paigasus/sdk/**'],
        message:
          "@paigasus/discovery must not depend on @paigasus/sdk: its `Presentation` union cannot express a DegradedReason, and a transitive edge would give @paigasus/app-shell a path to the sdk that `paigasus/boundaries/app-shell` bans (spec F8, § 2.1).",
      },
    ]),
  },
```

Then add `'discovery'` to the `BOUNDARY_SCOPES` list in `ts/packages/paigasus-next-config/tests/boundaries.test.ts`. Read the file first to match its exact shape; the liveness test pairs each declared scope with a rule, so a scope with no block (or a block with no scope) fails.

- [ ] **Step 3: Run the boundary tests**

```bash
pnpm -C ts/packages/paigasus-next-config exec vitest run tests/boundaries.test.ts
```

Expected: PASS.

- [ ] **Step 4: Derive the new expected sets — do NOT type them by hand**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-509

# The project-level case.
printf '%s\n' contracts/proto/paigasus/gateway/v1/health.proto \
  | moon query projects --affected --downstream deep

# The three task-level cases. Same no-flag traversal `_assert_task_case_impl` uses.
for f in \
  ts/packages/paigasus-proto/src/generated/paigasus/common/v1/error_pb.ts \
  ts/packages/paigasus-proto/src/generated/paigasus/iam/v1/iam_pb.ts \
  ts/packages/paigasus-discovery/src/core/state.ts \
  ts/packages/paigasus-discovery/src/adapters/memory-cache.ts
do
  echo "=== $f ==="
  printf '%s\n' "$f" | moon query tasks --affected
done
```

Record the output. The next step edits `run.sh` from these measurements only.

- [ ] **Step 5: Re-baseline `run.sh` and add the two new cases**

In `ci/affected-graph/run.sh`:

1. Add `paigasus-discovery-ts` to the `contracts->proto` expected set (line ~269) and extend the comment above it to say why: `@paigasus/discovery` consumes the generated `ServiceInfo`/`Capability` types.
2. Add `paigasus-discovery-ts:build,paigasus-discovery-ts:test` to both the `proto->sdk` and `proto-iam->sdk` expected sets.
3. Correct the stale premise at lines 89–91. It currently records as MEASURED that "`paigasus-auth-ts` is the only project declaring `test-e2e`". Replace with: two projects now declare it (`paigasus-auth-ts` and `paigasus-discovery-ts`); no existing case's observed set changes because their touched files differ, but a future case must re-check.
4. Add the two-anchor pair, placed after the `proto-iam->sdk` case:

```bash
  # SMA-509 — a @paigasus/discovery SOURCE edit must select its own build/test AND the
  # Docker-backed `test-e2e` task, plus `ts:lint`. Nothing else asserts this package's tasks are
  # reachable from an edit to it: `repo:input-liveness` scans `repo:*` tasks only and proves
  # DECLARED inputs are live, never that NEEDED ones are declared.
  # The expected set is DERIVED with the same no-flag `moon query tasks --affected` traversal
  # `_assert_task_case_impl` uses, not typed by hand.
  run_task_case_ci "discovery->discovery-tasks" "ts/packages/paigasus-discovery/src/core/state.ts" \
    "paigasus-discovery-ts:build,paigasus-discovery-ts:test,paigasus-discovery-ts:test-e2e,ts:lint"
  # The SECOND anchor, and it is not redundant. The case above anchors only on src/core/, so
  # narrowing the inherited `sources` file group to `src/core/**/*` would leave it green while an
  # edit to an ADAPTER stopped selecting anything — and the adapters are where the Redis lock
  # primitives live, which is exactly what the test-e2e task exists to exercise. Two anchors on
  # opposite sides of the glob prove its WIDTH rather than one path inside it. This repo's
  # precedent is the `ui->console` / `ui-components->console` pair.
  run_task_case_ci "discovery-adapters->discovery-tasks" "ts/packages/paigasus-discovery/src/adapters/memory-cache.ts" \
    "paigasus-discovery-ts:build,paigasus-discovery-ts:test,paigasus-discovery-ts:test-e2e,ts:lint"
```

Replace every expected set above with the **measured** output from Step 4 if it differs.

- [ ] **Step 6: Correct the `ci.yml` comment**

In `.github/workflows/ci.yml` around line 185, the Playwright install step is annotated "(for `@paigasus/auth`'s test-e2e)". Two projects now declare `test-e2e`. Update the comment only; the step itself is unchanged. `@paigasus/discovery`'s `test-e2e` runs vitest containers and no Playwright.

- [ ] **Step 7: Run the affected-graph gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
# Use the SYSTEM bash: bash 5.3.15 on this machine deadlocks forever on a `while read` fed by a
# here-string over ~512 bytes, which hangs this gate locally while CI stays green. A hang is NOT
# a gate failure.
/bin/bash ci/affected-graph/run.sh
```

Expected: PASS on every case.

- [ ] **Step 8: Commit**

```bash
git add ts/packages/paigasus-discovery/moon.yml ts/packages/paigasus-next-config ci/affected-graph/run.sh .github/workflows/ci.yml
git commit -m "ci(ts): wire paigasus-discovery into Moon, eslint boundaries and the affected graph (SMA-509)"
```

---

### Task 12: Full-graph verification

**Files:** none created; this task fixes whatever the gates report.

- [ ] **Step 1: Format and lint**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-509
moon run ts:fmt
moon run ts:lint
```

`ts:fmt` is a separate whole-tree Prettier gate, decoupled from `ts:lint`. Run it after the last TypeScript edit.

- [ ] **Step 2: Run the package's own tasks**

```bash
moon run paigasus-discovery-ts:build paigasus-discovery-ts:typecheck paigasus-discovery-ts:test
moon run paigasus-discovery-ts:test-e2e
```

- [ ] **Step 3: Run the full CI graph exactly as CI does**

Per-project tasks do **not** run the repo-level gates. This is the command CI runs:

```bash
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :test-e2e \
  --base origin/main \
  --include-relations
```

- [ ] **Step 4: Diagnose any failure before re-running**

**Capture before you re-run anything.** A re-run overwrites the evidence, and a *passing* re-run is just as destructive: it rewrites `stdout.log`, truncates `stderr.log`, rewrites `lastRun.json`, and flips the action to `passed` in `ciReport.json`.

```bash
cp .moon/cache/ciReport.json /tmp/sma509-ciReport.json
jq '.actions[] | select(.status=="failed")
    | {label, error,
       exec: (.operations[] | select(.meta.type=="task-execution") | {command, exitCode})}' \
   .moon/cache/ciReport.json
cat .moon/cache/states/<project>/<task>/stdout.log
cat .moon/cache/states/<project>/<task>/stderr.log
```

There is no action-level `exitCode` key; the real code and command are in `operations[]`.

Two known false alarms:
- A sub-3s `repo:affected-smoke` abort under a concurrent `moon ci`, whose output contains `proto-shim: … Permission denied (os error 13)`. Grep for that line. If present, re-run `moon run repo:affected-smoke --force`; it passes in the usual 6s.
- A local hang in `affected-smoke` or `actionlint` is this machine's bash 5.3.15 here-string deadlock, not a gate failure. Re-run under `/bin/bash`.

- [ ] **Step 5: Fix, re-run, and commit any fixes**

```bash
git add -A
git commit -m "fix(ts): address full-graph gate findings for @paigasus/discovery (SMA-509)"
```

---

## Self-review

**Spec coverage.** §4 package shape → Task 1, 3, 4, 5, 6, 7, 8. §5 configuration → Task 6. §6 cache record → Task 2. §7 probe → Task 4. §8 resolution, including all five invariants and the fenced write → Task 5. §9 public API → Tasks 6 and 8. §9.5 memoization → Task 6. §9.6 disabled rendering → Task 8. §10 reasons → Task 4. §11 AC1 → Task 8; AC2 → Task 5; AC3 → Task 5 (fast + never-settling) and Task 10 (real Redis, cross-process); AC4 → Task 9. §12 gate obligations 1–9 → Task 11 and Task 12.

**Type consistency.** `ServiceDescriptor`, `DegradedReason`, `ServiceState`, `CacheRecord`, `Timings`, `ProbeOutcome`, `DescriptorCache`, `DiscoveryLogger`, `Discovery` are each defined once and referenced with the same names and shapes throughout. `resolveService(deps, service, token)` and `probeService(opts)` keep one signature across all tasks.

**Task ordering.** Tasks are strictly sequential, 1 through 12. The one forward reference — `server.ts` re-exporting the Redis adapter — is resolved by Task 6 leaving a NOTE comment in its place and Task 7 Step 5 replacing that comment with the real line. No task depends on a file a later task creates.
