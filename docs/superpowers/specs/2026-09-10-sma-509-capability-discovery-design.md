# SMA-509 — `@paigasus/discovery`: capability discovery with three service states

**Status:** approved design
**Date:** 2026-09-10
**Issue:** [SMA-509](https://linear.app/smaschek/issue/SMA-509/ts-capability-discovery-client-with-three-service-states)
**ADR:** ADR-0020 — Service capability discovery, including its 2026-08-15 amendment (A1–A5)
**Depends on:** SMA-505 (descriptor, Done), SMA-508 (`@paigasus/sdk`, Done)
**Blocks:** SMA-510 (`@paigasus/app-shell`)

---

## 1. Problem

`paigasus-core` is self-hostable. A deployment may run IAM alone, IAM and the
gateway, or more services later. The console must adapt at runtime.

Three properties need different mechanisms:

- **Configured** — the operator deployed the service and supplied its address.
  Only the deployment knows this.
- **Reachable** — the service answers now.
- **Capable** — which features that build supports.

Addresses come from deployment configuration. Capabilities come from the service
itself, through the `ServiceInfo` descriptor that SMA-505 landed.

---

## 2. Scope

This issue delivers **package code only**. It does not wire authentication or
discovery into `paigasus-console`.

The console today depends on `@paigasus/next-config` and `@paigasus/ui` only. It
has no `middleware.ts`, no auth route handler, and no `getSession()` call, so
`@paigasus/auth` (SMA-506) is not wired into the app. Discovery must run inside
an authenticated request context, so it cannot run in the console as it stands.

SMA-510 owns navigation and is blocked on the interface this issue defines. The
app wiring belongs there.

**In scope:** the package, its public API, its cache, its tests, and the repo
gate changes the new package forces.

**Out of scope:** console wiring, the OIDC flow, a navigation component, and the
choice of whether production shares one Redis connection with `@paigasus/auth`.

---

## 3. Facts established before design

These come from the code and the ADRs. They are not open questions.

**F1 — The transport is HTTP for both services.**
`contracts/proto/paigasus/common/v1/service_info.proto` defines
`ServiceInfoService.GetServiceInfo`. IAM serves it over gRPC and over
`GET /v1/service-info`. The gateway has no gRPC server at all; it is axum-only
and serves `GET /v1/service-info` alone. ADR-0020 A5 states that serving HTTP on
both is what lets the console read both descriptors without a gRPC transport
stack. One call shape covers both services.

**F2 — Both routes are authenticated but not authorized.**
IAM's route sits in the `protected` sub-router behind `require_bearer`. The
gateway's uses `require_authenticated`, not `require_iam_auth`. Per ADR-0020 A4,
a validated token whose identity is not yet provisioned counts as authenticated
on the discovery path. The descriptor is byte-identical for every caller.

**F3 — `version` carries no signal.**
Every crate in `rs/` is `version = "0.0.0"` and `rs/release-plz.toml` is dormant,
so `ServiceInfo.version` reports `0.0.0` on every deployment. The console must
not render a version-skew banner. Capability keys are the only sanctioned input
to a feature decision.

**F4 — A disabled capability is indistinguishable from an old build.**
Per ADR-0020 A2, a disabled capability's HTTP routes are not registered (`404`)
and its gRPC RPCs return `UNIMPLEMENTED`. The client cannot tell "turned off"
from "not built yet", and does not need to.

**F5 — `@paigasus/auth` owns the only Redis connection.**
`createRedisSessionStore()` runs once inside a memoized `createAuthRuntime()`.
Its `SessionStore` port is session-shaped (`get`, `set`, `delete`,
`tryAcquireLock`, `releaseLock`, `putTransaction`, `takeTransaction`), and
`core/single-flight.ts` is written against `SessionRecord`. Neither is a general
cache, so neither is reused directly.

**F6 — There is no UI pattern to inherit.**
`@paigasus/ui` has no Button, no Tooltip, and no "disabled with a reason"
component. Radix's Tooltip primitive ships inside the already-installed
`radix-ui` package but is unused.

---

## 4. Package shape

A new source-only package, `private: true`, no build step, matching every other
package in `ts/packages/`.

```
ts/packages/paigasus-discovery/
  src/
    server.ts                    './server' entry, 'server-only'
    react.tsx                    './react' entry, 'server-only'
    types.ts                     './types' entry, client-safe
    config.ts                    discoveryEnvShape (zod)
    probe.ts                     GET /v1/service-info
    has-capability.ts
    core/
      state.ts                   ServiceState, DegradedReason
      record.ts                  CacheRecord, freshness predicates
      single-flight.ts           SWR + lock algorithm
    ports/
      cache.ts                   DescriptorCache
    adapters/
      redis-cache.ts             takes an injected node-redis client
      memory-cache.ts
```

### 4.1 Entry points

| Entry | Contents | Guard |
|---|---|---|
| `./server` | `createDiscovery`, `getServiceState`, `hasCapability`, `DescriptorCache`, both adapters, `discoveryEnvShape` | `import 'server-only'` |
| `./react` | `<Capability>` | `import 'server-only'` — it is an async server component |
| `./types` | `ServiceState`, `DegradedReason`, `SERVICE_STATES` | none |

`./types` carries no guard so a client component can hold the state as data.
This mirrors `@paigasus/sdk`'s `./errors/types`, which
`tests/server-guard.test.ts` there already pins as a deliberate exception.

There is **no root `"."` export.** `@paigasus/auth` omits one for a documented
reason: a root export re-exporting the server surface lets a client import route
around the boundary. The same reasoning applies here.

### 4.2 Dependencies

| Package | Why |
|---|---|
| `@paigasus/proto` | `ServiceInfoSchema` for `fromJson`, `capabilityWireKey` for the registry vocabulary |
| `@paigasus/sdk` | `mapError`, so a degraded reason derives from the `(domain, reason)` vocabulary rather than message text |
| `redis` | the Redis cache adapter (node-redis v6, as `@paigasus/auth` uses) |
| `zod` | `discoveryEnvShape` |
| `server-only` | the boundary guard |
| `react` (peer) | `<Capability>` |

### 4.3 Redis by injection

The package never opens a connection. `createRedisDescriptorCache(client,
keyPrefix)` takes an **already-connected** node-redis client.

This defers a decision that does not belong to this issue: whether production
shares `@paigasus/auth`'s client or opens a second one. The composition root
decides when the console is wired in SMA-510.

---

## 5. Configuration

`discoveryEnvShape` is a zod shape the app composes into its existing
`defineRuntimeConfig()` call, the same way `authEnvShape` is intended to be.
`ts/apps/paigasus-console/app/runtime-config.ts` already carries a comment
anticipating exactly this composition.

```
PAIGASUS_SERVICES = {"iam":"http://iam:8080","gateway":"http://gateway:8080"}
```

A JSON object mapping service name to base URL, parsed as
`@paigasus/next-config` already parses `PAIGASUS_ZONES` into
`Record<string, string>`.

**A service absent from this map is in the `absent` state.** This is the only
source of the `absent` state; no probe is involved.

These are cluster-internal DNS names. They stay in the private
`getRuntimeConfig()` slice and must never appear in `getPublicConfig()`.

Additional variables, all with defaults:

| Variable | Default | Meaning |
|---|---|---|
| `PAIGASUS_DISCOVERY_FRESH_MS` | 60000 | inside this, serve and do not probe |
| `PAIGASUS_DISCOVERY_STALE_MS` | 600000 | the Redis hard TTL; the stale window |
| `PAIGASUS_DISCOVERY_NEGATIVE_MS` | 10000 | how long a failure is remembered |
| `PAIGASUS_DISCOVERY_PROBE_TIMEOUT_MS` | 1500 | per-probe deadline |
| `PAIGASUS_DISCOVERY_LOCK_TTL_MS` | 5000 | single-flight lock lifetime |

`createDiscovery` asserts `PROBE_TIMEOUT_MS < LOCK_TTL_MS` at construction. The
lock must outlive the probe it guards. `@paigasus/auth`'s `createAuthRuntime`
makes the equivalent assertion for its own two IdP calls.

---

## 6. The cache record

```ts
type CacheRecord = {
  readonly descriptor: ServiceInfo | null;   // last GOOD probe
  readonly descriptorAt: number;
  readonly outcome: 'ok' | 'fail';           // last probe ATTEMPT
  readonly outcomeAt: number;
  readonly reason: DegradedReason | null;
};
```

Reachability and the capability list are **independent fields**. This is the
central design decision. A failed probe sets `outcome: 'fail'` but leaves
`descriptor` untouched, so the service reports `degraded` while still serving
its last known feature set. The UI can then render the right items, each
disabled with a reason, and the outage stays visible.

The rejected alternative — treating a servable stale record as `available` —
hides an outage for the whole stale window. ADR-0020 calls that "the worst
possible presentation for an operator".

### 6.1 Keys

```
<prefix>svcinfo:<service>
<prefix>svcinfo:lock:<service>
```

The issue specifies `svcinfo:<service>`. The prefix is injected and is `''` in
production, matching `@paigasus/auth`, where a per-test random prefix isolates
tests sharing one container.

### 6.2 Three TTLs

One TTL cannot express stale-while-revalidate. A 60-second Redis expiry deletes
the record we want to serve stale.

| | Default | Role |
|---|---|---|
| Fresh | 60s | a logical age carried in the record, not a Redis TTL |
| Hard | 10 min | the Redis `PX` expiry; past this the next request is cold |
| Negative | 10s | a failure's freshness, so a down service does not re-probe on every request |

Freshness is computed from `descriptorAt` and `outcomeAt` against the clock, not
from key existence.

---

## 7. The probe

```
GET {baseUrl}/v1/service-info
Authorization: Bearer <access token from the request-scoped session>
```

Under `AbortSignal.timeout(PROBE_TIMEOUT_MS)`. The body is canonical protojson of
the **bare** `ServiceInfo` message — not the `GetServiceInfoResponse` wrapper —
and is parsed with protobuf-es `fromJson(ServiceInfoSchema, body)`. Both services
return `capabilities` as `[]` when empty rather than omitting it.

Parsing through the generated schema, rather than by hand, means an unknown
field is ignored by protobuf-es itself. That is decision 6's version-skew rule
satisfied by the codec rather than by our own code.

The bearer token is supplied per call. It is never stored in the cache record and
never used as part of a cache key. The descriptor is identical for every caller
(F2), so one cache entry serves all users.

---

## 8. Resolution

`getServiceState(service, token)` resolves one service.

1. **Not in `PAIGASUS_SERVICES`** → `absent`. Return immediately. No Redis, no
   probe.
2. **Record exists and is fresh** → return it. No lock, no probe.
3. **Record exists and is stale** → return it **immediately**, then attempt the
   lock and revalidate **without awaiting**. This is what satisfies AC2.
4. **No record (cold)** → take the lock and probe, bounded by the probe timeout.

### 8.1 The background revalidation

Step 3's promise is handed to an injected
`waitUntil?: (p: Promise<unknown>) => void`. When the app supplies Next's
`after`, the runtime keeps the process alive until the probe settles. When it
does not, the promise floats with a `.catch()` attached.

Injection is what keeps `next/*` out of this package. Only `@paigasus/ui` carries
a hard no-`next/*` rule, but taking a `next` dependency here would be gratuitous.

### 8.2 The single-flight lock

`SET <lockKey> <token> NX PX <LOCK_TTL_MS>`, with a unique token per attempt and
a compare-and-delete release inside a `finally`. `@paigasus/auth`'s
`redis-store.ts` already carries the release script; this package keeps its own
copy rather than importing across a package boundary that exposes a session
store.

A loser on the cold path polls with backoff and jitter for the winner's record,
bounded at the probe timeout. On the deadline it returns `degraded` with reason
`timeout`. A loser **never** probes as a fallback — that is invariant 2 of
`core/single-flight.ts`, and it is the property AC3 tests.

After acquiring the lock, the holder re-reads the record before probing. Another
holder may have finished between the failed read and the successful acquire.

Worst-case added render latency is **one probe timeout**, whatever the number of
unreachable services, because a cold loser waits on the winner and not on its own
probe.

---

## 9. Public API

### 9.1 `ServiceState`

```ts
type ServiceState =
  | { readonly state: 'absent';
      readonly service: string }
  | { readonly state: 'available';
      readonly service: string;
      readonly descriptor: ServiceInfo;
      readonly capabilities: ReadonlySet<string> }
  | { readonly state: 'degraded';
      readonly service: string;
      readonly reason: DegradedReason;
      readonly descriptor: ServiceInfo | null;
      readonly capabilities: ReadonlySet<string> };
```

`degraded` carries `descriptor: ServiceInfo | null` because a service may fail
before any successful probe. `capabilities` is then empty.

### 9.2 `hasCapability`

```ts
hasCapability(key: string, token: string): Promise<boolean>
```

The service is derived from the key's first dot-segment: `iam.audit` resolves
against `iam`, `gateway.chat.stream` against `gateway`. The registry's naming
already guarantees this, and `capabilityWireKey` derives the same strings from
the enum.

Returns `true` **only** when the state is `available` and the key is present.
`degraded` returns `false`, because the feature must not be invoked. An unknown
key returns `false` without error — decision 6's "unknown key → ignore".

### 9.3 `<Capability>`

```tsx
<Capability need="iam.audit">
  <NavLink href="/audit">Audit</NavLink>
</Capability>
```

It branches on the **full state**, not on `hasCapability`:

| Situation | Render |
|---|---|
| service absent | nothing |
| available, key present | children |
| available, key absent | nothing |
| degraded, last descriptor had the key | children, disabled with a reason |
| degraded, no descriptor ever | children, disabled with a reason |

**The last row is a decision, not a derivation.** We do not know whether a
never-reached service has the capability. Rendering it disabled is chosen over
hiding it, because hiding a deployed-but-down service is the exact failure
ADR-0020 names, and a wrongly-shown disabled item is recoverable while a
wrongly-hidden one is invisible.

The `available, key absent` row hides rather than disables, and that asymmetry is
deliberate: the service answered and told us it does not have the feature. There
is no outage to report.

### 9.4 The disabled rendering

The default wrapper:

```html
<span aria-disabled="true" tabindex="-1" aria-describedby="cap-r1"
      class="pointer-events-none opacity-50">
  <a href="/audit">Audit</a>
  <span id="cap-r1" class="sr-only">IAM is not answering</span>
</span>
```

The component cannot reach into arbitrary children to set a `disabled` prop, so
it wraps them.

**It deliberately does not use `inert`.** The `inert` attribute removes the node
from the accessibility tree. That hides the item from a screen reader and breaks
"never hidden" for exactly the users least able to recover from it.
`aria-disabled` plus `tabindex="-1"` keeps the item announced while removing it
from the tab order.

A `degraded` render prop overrides the wrapper:

```tsx
<Capability need="iam.audit" degraded={(reason) => <NavItem disabled reason={reason} />}>
```

SMA-510's navigation is the expected first user of that override.

---

## 10. Degraded reasons

A closed union, never free text:

| Code | Cause |
|---|---|
| `timeout` | the probe exceeded its deadline, or a cold loser hit the lock-wait deadline |
| `network` | connection refused, DNS failure, TLS failure |
| `unauthorized` | `401` or `403` |
| `not-implemented` | `404`, meaning a service that predates the descriptor |
| `bad-response` | a 2xx body that is not valid `ServiceInfo` protojson |
| `server-error` | `5xx` |

Each is carried with a human string for display. **The UI branches on the code
only.** That is the discipline `@paigasus/sdk`'s `mapError` already enforces, and
the reason this package takes the SDK dependency: the mapping from a failed HTTP
call to a code runs through `mapError`'s `'http'` and `'transport'` arms rather
than through a second hand-written table.

---

## 11. Testing

Each acceptance criterion gets its own test. None is inferred from a neighbour.

### AC1 — three states, independently

Four tests, not three:

1. `absent` — service not in `PAIGASUS_SERVICES` renders nothing.
2. `available` — configured and answering renders children normally.
3. `degraded` with a cached descriptor — renders children, disabled, with a
   reason.
4. `degraded` with no descriptor — renders children, disabled, with a reason.

Tests 3 and 4 assert the rendered output **carries** `aria-disabled="true"` and
an accessible reason, and assert it is **not** `inert` and **not** absent from
the tree. Asserting only "it rendered something" would pass an implementation
that hides the reason.

### AC2 — a slow service does not block

A probe that never settles, against a stale record, must return within a few
milliseconds carrying the cached descriptor. The assertion is on elapsed time and
on the returned descriptor's identity, so an implementation that awaits the probe
fails on the clock rather than on the value.

### AC3 — exactly one probe

Both halves:

- **In-memory** — many concurrent `getServiceState` calls against
  `createMemoryDescriptorCache()` with a counting probe. The counter must read
  exactly 1. Runs on every PR.
- **Real Redis** — the same assertion against `redis:8-alpine` via
  `testcontainers`, under a separate `test-e2e` Moon task with its own
  `vitest.containers.config.ts`. This is the half a fake cannot prove: that
  `SET NX PX` and compare-and-delete are atomic.

One shared contract suite runs against **both** adapters, mirroring
`@paigasus/auth`'s `tests/store-contract.ts`, so a semantic divergence between
the memory and Redis adapters fails in CI rather than in production.

**No skip hatch when Docker is unreachable.** The containers suite fails loudly.
`@paigasus/auth` set this precedent deliberately, against the pattern
`paigasus-iam`'s Docker suites use.

### AC4 — gating is cosmetic

Not prose alone. A test asserts that a call made **despite** a missing
capability — a `404` and an `UNIMPLEMENTED` — maps to a clean `PaigasusError`
through `mapError`. That makes "the server remains authoritative" a checked
claim.

The README states it as well, and states that `hasCapability` is not a security
boundary.

### Test infrastructure

- vitest, `environment: 'node'` for the resolution tests.
- `<Capability>` tests need jsdom plus testing-library.
- `resolve.conditions` **and** `ssr.resolve.conditions` both set to
  `['react-server', 'node', 'import', 'default']`. vitest 5 resolves a
  node-environment test's imports through `ssr.resolve.conditions`, and
  `server-only` resolves to an unconditional throw without the `react-server`
  condition. `@paigasus/sdk` and `@paigasus/next-config` both carry this already.
- The probe is faked by injecting a `fetch`-shaped function, not by patching
  globals.

---

## 12. Repo gate obligations

The new package forces changes that red CI if they are missed. They are listed
here so they enter the plan as steps rather than as surprises.

1. **`proto->sdk` and `proto-iam->sdk` must be re-baselined.**
   `ci/affected-graph/run.sh` uses **strict equality**. Because this package
   depends on `@paigasus/proto`, an edit to a generated proto file now also
   selects `paigasus-discovery-ts:{build,test}`, and both cases fail on any extra
   project. The Frontend Architecture document flagged this class in § 7.1: "it
   belongs in the plan as an explicit step, not as a surprise."

2. **A two-anchor case pair for the new package.** One case alone lets an
   `inputs` glob be narrowed to the other subtree while staying green. The
   `ui->console` / `ui-components->console` pair is this repo's precedent, and
   `proto->sdk` / `proto-iam->sdk` its second. Anchor one case on `src/core/` and
   one on `src/adapters/`.

3. **A case covering `test-e2e`.** `@paigasus/auth`'s `auth->auth-tasks` case is
   the model: it asserts a source edit selects `build`, `test`, `test-e2e` and
   `ts:lint`.

4. **`:test-e2e` is already in `ci.yml`'s `T=(…)` array** and in CLAUDE.md's
   marker-delimited command. The new task joins an existing target, so there is
   **no** `T`-array obligation and no CLAUDE.md marker edit.

5. **Expected sets are derived, not typed.** Each is produced with the same
   no-flag `moon query tasks --affected` traversal `_assert_task_case_impl` uses,
   as the comments on the existing cases require.

6. **`ts:fmt` is a separate whole-tree Prettier gate**, decoupled from `ts:lint`.
   Run it after the last TypeScript edit.

---

## 13. Decisions rejected

| Rejected | Why |
|---|---|
| Put discovery in `@paigasus/sdk` | The SDK would gain a `redis` dependency and a caching subsystem, making it infrastructure rather than typed clients over proto. It has no `react` dependency, so `<Capability>` would still need a home. |
| Put discovery in `@paigasus/auth` | Service topology is not identity. The `SessionStore` port is session-shaped and would have to grow a general cache it does not want. |
| Split across sdk, a new package, and ui | Most faithful to the documented dependency rules, but spreads one feature across three packages and forces `<Capability>` to take a resolved `state` prop rather than the `need=` prop the issue specifies. |
| gRPC for IAM, HTTP for the gateway | Two code paths where one works. ADR-0020 A5 chose HTTP on both for exactly this reason. |
| Serve a stale record as `available` | Hides an outage for the whole stale window. |
| A failed probe drops the capability list | Defeats AC2: a blipping service collapses the whole navigation instead of showing known-but-disabled items. |
| Return `degraded` immediately on a cold cache | The first page load after a deploy would always show every service as down, which is itself a false ops signal. |
| Block until the probe settles, with no deadline | Cold render latency becomes the slowest service's timeout, and several unreachable services add up. |
| `inert` on the disabled wrapper | Removes the node from the accessibility tree, hiding it from a screen reader. |
| A React context plus `useCapability()` | Forces children into client components, which defeats server resolution and pushes the capability list into the browser bundle. |

---

## 14. Open items for the implementation plan

- The exact re-baselined expected sets for `proto->sdk` and `proto-iam->sdk` must
  be **measured**, not predicted.
- `@paigasus/discovery` needs a `moon.yml` whose `test` and `build` tasks list
  `/ts/packages/paigasus-proto/src/**/*` and `/ts/packages/paigasus-sdk/src/**/*`
  among their `inputs`. Nothing else confers affectedness in Moon 2.5.3.

### 14.1 Resolved before planning

Two items that looked open are settled, and both remove work:

- **No `pnpm-workspace.yaml` package edit.** It globs `packages/*` and `apps/*`,
  so a new directory is picked up with no manifest change.
- **No catalog additions.** Every dependency this package needs is already
  catalog-pinned: `redis ^6.2.1`, `testcontainers ^12.1.0`, `zod ^4.5.4`,
  `server-only ^0.0.1`, `jsdom ^30.0.1` and the four `@testing-library/*`
  entries. This matters beyond convenience — pnpm 11's 24-hour
  `minimumReleaseAge` reds CI on a same-day npm release, and adding no new
  catalog entry avoids that failure mode entirely.
