# SMA-509 — `@paigasus/discovery`: capability discovery with three service states

**Status:** approved design, revision 2 (after adversarial challenge)
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

**In scope:** the package, its public API, its cache, its tests, and the repo
gate changes the new package forces.

**Out of scope:** console wiring, the OIDC flow, a navigation component, and the
choice of whether production shares one Redis connection with `@paigasus/auth`.

### 2.1 How SMA-510 consumes this

`<Capability>` is an **async server component**. The ESLint block
`paigasus/boundaries/app-shell` (`ts/packages/paigasus-next-config/src/eslint.mjs:107-116`)
bans `@paigasus/sdk` and `@paigasus/auth/server` from `@paigasus/app-shell`
because app-shell is client-reachable.

Therefore **app-shell must not import `@paigasus/discovery/react`.** The division
is:

- `@paigasus/app-shell` exports navigation **presentation** that takes resolved
  state as props. It stays client-reachable.
- The **app** composes `<Capability>` around app-shell's presentation
  components, because only the app has a server request context.

This constraint is recorded here so SMA-510 does not discover it late.

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

**F3 — `version` carries no signal for these two services.**
`paigasus-gateway` and `paigasus-iam` are pinned at `version = "0.0.0"`
**deliberately**: `env!("CARGO_PKG_VERSION")` feeds `ServiceInfo`, and ADR-0020
skew reporting is parked on that value (SMA-505 R7). release-plz has since cut
its first live release (SMA-580), but it tagged only the kernel family, not
these crates. The console must not render a version-skew banner. Capability keys
are the only sanctioned input to a feature decision.

**F4 — A disabled capability is indistinguishable from an old build.**
Per ADR-0020 A2, a disabled capability's HTTP routes are not registered (`404`)
and its gRPC RPCs return `UNIMPLEMENTED`. The client cannot tell "turned off"
from "not built yet", and does not need to.

**F5 — `@paigasus/auth` owns the only Redis connection.**
`createRedisSessionStore()` runs once inside a memoized `createAuthRuntime()`.
Its `SessionStore` port is session-shaped and `core/single-flight.ts` is written
against `SessionRecord`. Neither is a general cache, so neither is reused
directly.

**F6 — There is no UI pattern to inherit.**
`@paigasus/ui` has no Button, no Tooltip, and no "disabled with a reason"
component.

**F7 — The cache key's `<service>` is fixed by a proto MUST.**
`service_info.proto:84-94` states that `ServiceInfo.service` is **advisory and
never a cache key**, and that the `<service>` in `svcinfo:<service>` "MUST be the
client's own deployment configuration identifier for the service it dialled,
never this server-reported value — otherwise a misconfigured or hostile service
could poison another service's cache entry. A mismatch is worth logging and
nothing more."

**F8 — `@paigasus/sdk`'s `Presentation` union cannot express a degraded reason.**
`ts/packages/paigasus-sdk/src/errors/types.ts:22` fixes it at
`'relogin' | 'forbidden' | 'not-found' | 'degraded' | 'rate-limited' |
'invalid-input' | 'conflict' | 'disabled' | 'generic'`. `HTTP_TABLE`
(`transport-status.ts:55-69`) has **no 500 row** and **deliberately no 2xx row**.
This package therefore owns its own reason vocabulary (§10) and takes **no**
`@paigasus/sdk` dependency. Revision 1 of this spec claimed otherwise and was
wrong.

---

## 4. Package shape

A new source-only package, `private: true`, no build step.

```
ts/packages/paigasus-discovery/
  README.md                      required by AC4
  moon.yml
  vitest.config.ts               node; jsdom per-file via docblock
  vitest.containers.config.ts    real-Redis project
  tests/support/server-only-stub.ts
  src/
    server.ts                    './server' entry, 'server-only'
    react.tsx                    './react' entry, 'server-only'
    disabled.tsx                 'use client' — the disabled wrapper
    types.ts                     './types' entry, client-safe
    config.ts                    discoveryEnvShape (zod)
    probe.ts                     GET /v1/service-info
    core/
      state.ts                   CAPABILITY_KEYS, SERVICE_SLUGS, serviceOf (imports @paigasus/proto)
      record.ts                  CacheRecord, isFresh, toState
      reasons.ts                 status/cause -> DegradedReason
      single-flight.ts           SWR + lock algorithm
    ports/
      cache.ts                   DescriptorCache
      logger.ts                  DiscoveryLogger
    adapters/
      redis-cache.ts             takes an injected node-redis client
      memory-cache.ts
      noop-logger.ts
```

**`ServiceState`, `DegradedReason` and `SERVICE_STATES` live in `types.ts`, not
`core/state.ts`.** An earlier revision of this section put them in
`core/state.ts`; the shipped code corrects that. `core/state.ts` imports
`@paigasus/proto` as a VALUE (to derive `CAPABILITY_KEYS` and `SERVICE_SLUGS`),
and `types.ts` is the client-safe entry — re-exporting these three from
`core/state.ts` would pull protobuf-es into any client bundle that imports
`@paigasus/discovery/types`.

### 4.1 Entry points

| Entry | Contents | Guard |
|---|---|---|
| `./server` | `createDiscovery`, `DescriptorCache`, `DiscoveryLogger`, both adapters, `discoveryEnvShape` | `import 'server-only'` |
| `./react` | `<Capability>` | `import 'server-only'` |
| `./types` | `ServiceState`, `ServiceDescriptor`, `DegradedReason`, `CapabilityKey`, `SERVICE_STATES` | none |

`./types` carries no guard so a client component can hold the state as data,
mirroring `@paigasus/sdk`'s `./errors/types`.

There is **no root `"."` export**, for the reason `@paigasus/auth` documents: a
root export re-exporting the server surface lets a client import route around
the boundary.

`src/disabled.tsx` carries `'use client'` and is imported by `react.tsx`. It is
**not** an entry point.

### 4.2 Dependencies

| Package | Why |
|---|---|
| `@paigasus/proto` | `Capability` + `capabilityWireKey`, to derive the closed key and service-slug vocabularies |
| `zod` | `discoveryEnvShape` |
| `redis` | the Redis cache adapter (node-redis v6) |
| `server-only` | the boundary guard |
| `react` (peer) | `<Capability>` |

**No `@paigasus/sdk` dependency.** Three independent reasons: its `Presentation`
union cannot express these reasons (F8); the probe is a bare `fetch`, so pulling
in `@connectrpc/connect` and `@connectrpc/connect-node` is pure cost; and
`@paigasus/app-shell` is banned from reaching the SDK, which a transitive edge
would violate (§2.1).

`@paigasus/proto` is used **only for type-level and enum-level vocabulary**, never
on the wire — see §7 for why the descriptor is parsed by hand.

### 4.3 Redis by injection, with stated preconditions

`createRedisDescriptorCache(client, options)` takes an **already-connected**
node-redis client. The composition root decides whether to share
`@paigasus/auth`'s client or open a second one.

The client **must** be created with the options `@paigasus/auth`'s
`redis-store.ts:236-246` documents as load-bearing:

| Option | Why |
|---|---|
| `disableOfflineQueue: true` | without it, an outage becomes hung requests instead of fast failures |
| `commandOptions.timeout` | bounds a command against an unresponsive server |
| an `on('error')` listener | node-redis crashes the process without one; it must never log the raw error, which embeds the DSN |

`createRedisDescriptorCache` asserts `disableOfflineQueue` and the presence of an
error listener at construction and throws otherwise. A precondition nobody checks
is a precondition nobody keeps.

---

## 5. Configuration

`discoveryEnvShape` is a zod shape the app composes into its existing
`defineRuntimeConfig()` call, as `ts/apps/paigasus-console/app/runtime-config.ts`
already anticipates in a comment.

```
PAIGASUS_SERVICES = {"iam":"http://iam:8080","gateway":"http://gateway:8080"}
```

### 5.1 Key and value validation

`PAIGASUS_ZONES`' transform is **not** reusable: `zoneMapFromJson` validates each
value with `canonicalBasePath` (`next-config/src/runtime.ts:89`), which is a
**path** validator, not a URL validator. This package writes its own.

**Keys** are validated against the closed set of service slugs derived from
`capabilityWireKey` over the `Capability` enum — this is what the
`@paigasus/proto` dependency is actually for. An unknown key **fails
construction**. An operator writing `{"IAM": ...}` or `{"gw": ...}` would
otherwise get a silently empty console, which §9.3 itself calls the worse
failure. Keys must also match `^[a-z][a-z0-9]*$`.

**Values** are validated as URLs: `http:` or `https:` only, no userinfo, no
query, no fragment. The parsed map is built on `Object.create(null)`, reusing the
prototype-pollution rule at `next-config/src/runtime.ts:63-75`.

**A service absent from this map is in the `absent` state.** This is the only
source of the `absent` state; no probe is involved.

These are cluster-internal DNS names. They stay in the private
`getRuntimeConfig()` slice and must never appear in `getPublicConfig()`.

Note that `describeIssues` (`next-config/src/runtime.ts:136-145`) renders
`issue.message` only for keys `@paigasus/next-config` itself owns, so a malformed
value surfaces as `PAIGASUS_SERVICES: custom`. `@paigasus/auth/src/config.ts:13-16`
records the same constraint. The README therefore carries the expected form of
every variable.

### 5.2 Timings

| Variable | Default | Meaning |
|---|---|---|
| `PAIGASUS_DISCOVERY_NEGATIVE_MS` | 10000 | how long a failed probe is honoured before re-probing |
| `PAIGASUS_DISCOVERY_FRESH_MS` | 60000 | how long a successful probe is honoured |
| `PAIGASUS_DISCOVERY_STALE_MS` | 600000 | the Redis hard TTL; past this, the next request is cold |
| `PAIGASUS_DISCOVERY_PROBE_TIMEOUT_MS` | 1500 | per-probe network deadline |
| `PAIGASUS_DISCOVERY_LOCK_WAIT_MS` | 2500 | how long a cold loser waits for the winner |
| `PAIGASUS_DISCOVERY_LOCK_TTL_MS` | 5000 | single-flight lock lifetime |

`createDiscovery` asserts two orderings at construction:

```
NEGATIVE_MS < FRESH_MS < STALE_MS
PROBE_TIMEOUT_MS < LOCK_WAIT_MS < LOCK_TTL_MS
```

The second is what stops the §8.2 false-outage: a loser whose deadline expires
before the winner can finish its probe **and** write the record would report a
healthy service as down on the first render after every deploy.

---

## 6. The cache record

```ts
type CacheRecord = {
  readonly version: 1;
  readonly rev: number;
  readonly descriptor: ServiceDescriptor | null;   // last GOOD probe
  readonly descriptorAt: number;
  readonly outcome: 'ok' | 'fail';                 // last probe ATTEMPT
  readonly outcomeAt: number;
  readonly reason: DegradedReason | null;
};
```

Reachability and the capability list are **independent fields**. This is the
central design decision. A failed probe sets `outcome: 'fail'` and leaves
`descriptor` untouched, so the service reports `degraded` while still serving its
last known feature set. The UI renders the right items, each disabled with a
reason, and the outage stays visible.

`version: 1` exists because during a rolling upgrade two console builds write the
same key. Both adapters treat a version mismatch or an unparseable value as
**absent, and delete it** — the pattern `SessionRecord` already uses
(`redis-store.ts:168-171`, `memory-store.ts:36-40`). Without it, a renamed
`DegradedReason` poisons the cache fleet-wide for the full 10-minute hard TTL.

`rev` exists for the fenced write in §8.3.

### 6.1 Keys

```
<prefix>pgs:svcinfo:<service>
<prefix>pgs:svcinfo:lock:<service>
```

The `pgs:` namespace matches `@paigasus/auth`'s `pgs:sess:` / `pgs:lock:` /
`pgs:txn:`. The prefix is injected and is `''` in production.

Per **F7**, `<service>` is the key from `PAIGASUS_SERVICES` — never
`descriptor.service`. When a probe returns a `service` field that disagrees with
the configured key, the record is still stored under the configured key and the
mismatch is **logged and otherwise ignored**, exactly as the proto directs.

### 6.2 Freshness

Freshness is computed from **`outcomeAt` in both arms**. `descriptorAt` only ever
moves on success and is carried for observability, never for the freshness
decision:

```ts
isFresh(rec, now) =
  rec.outcome === 'ok'
    ? now - rec.outcomeAt < FRESH_MS
    : now - rec.outcomeAt < NEGATIVE_MS;
```

Using `descriptorAt` for the `ok` arm would make the negative TTL dead: a probe
that failed at t=0 over a descriptor from t=−5s would not be re-probed for a full
60s. Revision 1 left this ambiguous between two windows that disagree.

### 6.3 Record to state

`toState(service, rec)` is total:

| `rec` | `outcome` | `descriptor` | `ServiceState` |
|---|---|---|---|
| absent from config | — | — | `absent` |
| `null` (cold) | — | — | resolved by §8, never mapped directly |
| present | `ok` | non-null | `available` |
| present | `ok` | `null` | impossible; treated as corrupt → deleted, re-probed |
| present | `fail` | non-null | `degraded`, capabilities from the descriptor |
| present | `fail` | `null` | `degraded`, capabilities empty |

**A stale record whose last outcome was `ok` maps to `available`.** This is what
AC2 requires. §13's rejected row is narrower than revision 1 stated, and is
restated there.

---

## 7. The probe

```
GET {baseUrl}/v1/service-info
Authorization: Bearer <access token from the request-scoped session>
Accept: application/json
```

Bounded by `AbortSignal.timeout(PROBE_TIMEOUT_MS)`, with:

- `redirect: 'error'` — `fetch` follows redirects by default, and following one
  would send the user's bearer token wherever a compromised or misconfigured
  service points.
- a **case-insensitive** `content-type` check for `application/json` before
  parsing. Case matters: an externally supplied header must be compared
  case-insensitively, and every fixture must vary the case.
- a response-size cap, enforced while reading, so an oversized body cannot be
  buffered inside the timeout window on a fast internal link.

### 7.1 Parsing

The body is canonical protojson of the **bare** `ServiceInfo` message — not the
`GetServiceInfoResponse` wrapper. Both services return `capabilities` as `[]`
when empty.

It is converted at the boundary into a **plain structural type**:

```ts
type ServiceDescriptor = {
  readonly service: string;
  readonly version: string;
  readonly capabilities: readonly string[];
};
```

Not the protobuf-es `ServiceInfo` message, for two reasons. It crosses the RSC
boundary as a prop, and a protobuf-es message carries `$typeName` and is not a
plain object. And `paigasus/boundaries/apps` (`eslint.mjs:118-126`) bans apps from
importing `@paigasus/proto` **including type imports**, so an app could not name
`ServiceInfo` to hold it.

Unknown fields are dropped by the conversion, which satisfies decision 6's
"unknown key → ignore" at the codec boundary. A body that does not match the
shape yields `bad-response`.

### 7.2 The token is caller state, not cached state

The descriptor is identical for every caller (F2), so **one entry serves all
users**. The auth *outcome* is not caller-independent, and this is the trap.

**`401` and `403` are caller-scoped and are never written to the cache.** They
return `degraded` with reason `unauthorized` to that request only, and leave the
record untouched. Otherwise one user with an expired cookie writes `fail` into
the shared record and disables the navigation for every user in the deployment
for the negative TTL — self-perpetuating on a low-traffic deployment, with no
operator signal.

An **empty or absent token** never probes at all. The cache is served read-only,
and a cold cache yields `degraded` with reason `unauthorized`.

---

## 8. Resolution

`getServiceState(service, token)` resolves one service.

1. **Not in `PAIGASUS_SERVICES`** → `absent`. No Redis, no probe.
2. **Cache read throws** → `degraded` with reason `cache-unavailable`. Never
   propagate. One Redis blip must not 500 every server component rendering
   navigation.
3. **Record fresh** (§6.2) → `toState`. No lock, no probe.
4. **Record stale** → return `toState` **immediately**, then attempt the lock and
   revalidate **without awaiting**. This is what satisfies AC2.
5. **Cold** → take the lock and probe (§8.2).

### 8.1 The background revalidation

**What "without awaiting" means, precisely.** The lock acquisition and the
invariant-1 double-check ARE awaited before the stale value returns; only the
**probe** is not. Revision 2 specified detaching all three inside one
un-awaited block, and that was measured wrong during implementation: the
detached chain suspends on the lock acquire, so `resolveService` returned before
the probe had started at all, and the revalidation never began on a request that
supplied no `waitUntil`.

The cost is stated plainly: a stale-serve request now makes three awaited cache
calls (`get`, `tryAcquireLock`, `get`) instead of one before it returns. The same
three calls happen either way — detaching only moves them off the response — and
AC2 is unaffected, because AC2 is about the *probe* not blocking render. The
fresh path, which is the overwhelmingly common one, remains a single `get`.

The probe's promise is handed to an injected
`waitUntil?: (p: Promise<unknown>) => void`. When the app supplies Next's `after`,
the runtime keeps the process alive until the probe settles. Otherwise the promise
floats with a `.catch()` that logs.

Injection is what keeps `next/*` out of this package.

If the lock is already held, the background task **returns immediately** rather
than polling — another caller is already revalidating, and a second waiter buys
nothing. A leaked lock is bounded by `LOCK_TTL_MS` (5s), not by the hard TTL.

The deploy target is self-hosted OCI containers (Frontend Architecture F8), so
the process survives the response and a floated promise completes. On a
freeze-at-response platform it would not, and the effective refresh interval
would degrade to the hard TTL. `waitUntil` is the supported answer there; the
README says so.

### 8.2 The single-flight lock

`SET <lockKey> <token> NX PX <LOCK_TTL_MS>`, unique token per attempt,
compare-and-delete release in a `finally` — and that `finally` is itself wrapped
in `try/catch`, so a store error at release time cannot mask the probe's real
outcome (`single-flight.ts:57-59, 165-174`).

After acquiring the lock the holder **re-reads** the record before probing.
Another holder may have finished between the failed read and the successful
acquire.

A cold loser polls with backoff and jitter, bounded at `LOCK_WAIT_MS`. On the
deadline it performs **one final cache read** before degrading
(`single-flight.ts:180` does the same), then returns `degraded` with reason
`timeout`. A loser **never** probes as a fallback — invariant 2.

Worst-case added render latency is one `LOCK_WAIT_MS`, provided services are
resolved concurrently (§9.5).

### 8.3 The fenced write — invariant 5

`@paigasus/auth` states it explicitly: a compare-and-delete release protects the
**lock**, never the **write** (`single-flight.ts:60-65`), which is why
`redis-store.ts:24-34` carries a `SET_CAS` Lua script.

Revision 1 dropped this, and the exposure is larger here than in auth: a
background revalidation has **no wall-clock bound**. `AbortSignal.timeout` bounds
the network wait only — not JSON parsing, not the cache write, not an event-loop
stall. A probe started at t=0 could resolve at t=30s and blindly overwrite a
successful probe written at t=5s with its own older failure, masking a healthy
service until the hard TTL.

Every write is a **compare-and-set on `rev`**, via a Lua script modelled on
`redis-store.ts:24-34`, and **the token is the `rev` from the snapshot taken
BEFORE the probe ran** — never a fresh read taken at write time. A losing writer
re-reads and returns the winner's state.

That distinction is the whole fence, and it is easy to get wrong. Re-reading the
record immediately before writing and using *that* read's `rev` makes the
compare-and-set agree with itself, so it never fences anything. Revision 2 of
this spec specified exactly that no-op, and it was caught by measurement during
implementation: a stale `network` failure overwrote a newer successful record at
`rev 9`, with no fence event emitted. The shape here now matches
`ts/packages/paigasus-auth/src/core/single-flight.ts`, which this section always
claimed as its model.

**A second rule, "discard a write whose probe started before the stored
`outcomeAt`", was specified in revision 2 and has been dropped.** Three reasons,
all measured: the `rev` CAS strictly subsumes it, since any newer write moves
`rev`; the CAS additionally catches a newer write carrying the *same*
`outcomeAt`, which the timestamp rule cannot; and the rule compares one
process's clock against a timestamp another process wrote, so clock skew between
two console replicas could discard a valid write.

### 8.4 The holder's own probe deadline

`AbortSignal.timeout` bounds the network wait inside `probeService`, but a
`probe` that never settles for any other reason would hang the lock holder's own
call forever — the waiters would time out correctly while the holder never
returned. The holder therefore races its probe against `PROBE_TIMEOUT_MS`
itself. A timed-out holder returns `degraded`/`timeout`, writes a `fail` record
(held for the negative TTL), and releases the lock.

---

## 9. Public API

### 9.1 Construction

```ts
createDiscovery(deps: {
  services: Readonly<Record<string, string>>;
  cache: DescriptorCache;
  logger?: DiscoveryLogger;
  fetch?: typeof globalThis.fetch;
  waitUntil?: (p: Promise<unknown>) => void;
  timings?: Partial<Timings>;
  now?: () => number;
}): Discovery
```

`Discovery` is `{ getServiceState, hasCapability }`. There is **no module-level
singleton**: the composition root builds a fresh handle **per request**. This is
the OPPOSITE lifetime from `@paigasus/auth`'s `createAuthRuntime()`, which is
ONE RUNTIME PER PROCESS — an earlier revision of this section drew the
analogy the wrong way round, and the shipped code corrects it. The
`getServiceState` memo (§9.5) never evicts an entry, because the handle is
meant to die with the request; a process-wide handle would instead replay the
first caller's outcome, including a rejected token, for the process's whole
life.
Revision 1 listed the three as sibling exports with signatures that took no
handle, which was internally inconsistent.

### 9.2 State

```ts
type ServiceState =
  | { readonly state: 'absent';
      readonly service: string }
  | { readonly state: 'available';
      readonly service: string;
      readonly descriptor: ServiceDescriptor;
      readonly capabilities: readonly string[] }
  | { readonly state: 'degraded';
      readonly service: string;
      readonly reason: DegradedReason;
      readonly descriptor: ServiceDescriptor | null;
      readonly capabilities: readonly string[] };
```

`capabilities` is `readonly string[]`, **not** `ReadonlySet`. A `Set` does not
survive the RSC server-to-client boundary. Membership tests go through a helper.

### 9.3 `hasCapability`

```ts
hasCapability(key: CapabilityKey, token: string): Promise<boolean>
```

`CapabilityKey` is a **hand-declared** closed union in `src/types.ts:62`, not a
derivation and not `string`, and not `` `${string}.${string}` `` either — that
template would accept the typo `iam.audits`, which at runtime returns `false`
forever and silently hides a navigation item, the "invisible" outcome §9.4
itself calls the worse failure. Hand-declaring the union re-opens a drift risk
against the proto registry; `tests/vocabulary.test.ts:40` closes it by
comparing the union's members against the registry-derived runtime list, so a
new capability added to the proto reds that test until the union is updated by
hand.

The service is derived from the key's first dot-segment. The registry guarantees
this (`service_info.proto:85-86`), and §5.1 validates the config keys against the
same derived vocabulary, so the two cannot drift.

Returns `true` **only** when the state is `available` and the key is present.
`degraded` returns `false`, because the feature must not be invoked.

`hasCapability` is for **non-UI callers**. A consumer writing
`if (await hasCapability(...))` around markup gets the hiding behaviour §9.4
argues against. The README says this next to the "not a security boundary" line.

### 9.4 `<Capability>`

```tsx
<Capability discovery={discovery} need="iam.audit" token={token}>
  <NavLink href="/audit">Audit</NavLink>
</Capability>
```

The shipped component (`src/react.tsx:12-19`) also requires `discovery` and
`token` props, both omitted from an earlier revision of this example: `token`
is per-request caller state (§7.2), and `discovery` is the request-scoped
handle from §9.1 — neither can come from a module-level singleton, which is
part of why one must not exist.

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
deliberate: the service answered and told us it lacks the feature. There is no
outage to report.

### 9.5 Request-scoped memoization

`getServiceState` is memoized **per handle**, via a plain closure `Map` keyed on
service and token — not React's `cache()`, and not an injected request-scoped
map. An earlier revision of this section described both of those instead; the
shipped code uses neither.

This is exactly why the handle's lifetime rule (§9.1) matters: the `Map` is
never evicted, because there is deliberately nothing to evict FOR — the whole
handle, `Map` included, is meant to die with the request. Calling
`createDiscovery` once per process instead of once per request would make the
`Map` outlive every individual request, so a second caller with a different
token would receive the identical (possibly rejected) result the first caller
got, and freshness, stale-while-revalidate and the negative TTL would all
become unreachable, since the memo answers before the cache is ever consulted.

Without the memo at all, a navigation with eight `<Capability>` items over two
services costs eight cache reads per render, and §8.2's latency bound does not
hold, because React renders server components in tree order and each sibling
would resolve serially. The bound is stated conditionally on concurrent
resolution for exactly this reason.

### 9.6 The disabled rendering

`src/disabled.tsx` is a **client component**. This is forced: an async server
component cannot attach event handlers, and the wrapper must block activation.

```html
<span data-capability-state="degraded" data-capability-reason="network"
      aria-disabled="true" aria-describedby=":r3:">
  <a href="/audit">Audit</a>
  <span id=":r3:" class="…visually hidden…">IAM is not answering</span>
</span>
```

Four decisions:

1. **It does not use `inert`.** The `inert` attribute removes the node from the
   accessibility tree, which hides the item from a screen reader and breaks
   "never hidden" for exactly the users least able to recover.
2. **`tabindex` is not inherited**, so `tabindex="-1"` on the wrapper would leave
   a nested `<a>` fully keyboard-activatable while `aria-disabled` made the tests
   pass. Activation is blocked with capture-phase `onClickCapture` and
   `onKeyDownCapture` handlers that call `preventDefault()` and
   `stopPropagation()`, plus `pointer-events: none` as an inline style. This is
   the whole reason the wrapper is a client component.
3. **The description id comes from `useId()`.** A hardcoded id emits duplicates
   when two `<Capability>` elements appear on one page.
4. **No Tailwind utility classes.** Tailwind v4's scan root is the working
   directory and Moon runs `next build` from the app's directory, so a package
   shipping utility classes needs an `@source` line in **every** consumer;
   forgetting it drops the classes silently and **only in a production build**.
   That is the SMA-503 failure class verbatim. The functional bits are inline
   styles; everything cosmetic is exposed as `data-capability-state` and
   `data-capability-reason` for the consumer to style. The package needs no
   `@source` entry and no `ci/tailwind-source` sentinel.

A `degraded` render prop overrides the wrapper entirely:

```tsx
<Capability need="iam.audit" degraded={(reason) => <NavItem disabled reason={reason} />}>
```

---

## 10. Degraded reasons

Owned by this package (F8), never free text:

| Code | Cause |
|---|---|
| `timeout` | the probe exceeded `PROBE_TIMEOUT_MS`, or a cold loser hit `LOCK_WAIT_MS` |
| `network` | connection refused, DNS failure, TLS failure, a refused redirect |
| `unauthorized` | `401` or `403`, or an absent token — **never cached** (§7.2) |
| `not-implemented` | `404`, meaning a service predating the descriptor |
| `bad-response` | a 2xx whose content-type or body does not match `ServiceDescriptor` |
| `server-error` | any other non-2xx, `500` included |
| `cache-unavailable` | the cache read or write threw (§8 step 2) |

Classification is a small table in `core/reasons.ts` keyed on HTTP status, plus
`AbortSignal`'s `TimeoutError` versus a `fetch` `TypeError` for
`timeout`/`network`. **The UI branches on the code only.**

---

## 11. Testing

### AC1 — three states, independently

Four tests, not three:

1. `absent` — service not in `PAIGASUS_SERVICES` renders nothing.
2. `available` — configured and answering renders children normally.
3. `degraded` with a cached descriptor.
4. `degraded` with no descriptor.

Tests 3 and 4 assert the output **carries** `aria-disabled="true"` and an
accessible reason; is **not** `inert`; is **present** in the accessibility tree;
and — the assertion revision 1 lacked — that **activating the child does not
navigate**. Asserting only that `aria-disabled` is present passes an
implementation whose link is still keyboard-activatable.

### AC2 — a slow service does not block

No wall-clock assertion. Inject a probe returning a deferred promise the test
**never settles**, then `await getServiceState(...)` and assert it resolves with
the cached descriptor **by identity**. An implementation that awaits the probe
hangs and fails deterministically at the suite timeout. A second assertion checks
the probe *was* called, so revalidation is proven to have been attempted.

### AC3 — exactly one probe

Three cases, because the fast case cannot see the invariant it claims to test:

1. **Fast probe, in-memory** — many concurrent calls, counter reads exactly 1.
2. **Never-settling probe, in-memory** — counter still exactly 1, **and** every
   loser returned `degraded` with reason `timeout`. Without this case the
   never-fall-back invariant is unobserved: with a fast probe the winner finishes
   before any loser reaches its deadline, so the fallback branch never executes
   and the counter reads 1 even for an implementation that does fall back.
3. **Real Redis, cross-process** — a forked worker, mirroring
   `@paigasus/auth`'s `tests/containers/single-flight-multiprocess.test.ts`. The
   multi-zone argument applies here identically: two console zones are two
   processes, not two async callers sharing one event loop, and only that case
   proves `SET NX PX` and compare-and-delete are atomic rather than merely
   serialized by one event loop.

One shared contract suite runs against **both** adapters, mirroring
`@paigasus/auth`'s `tests/store-contract.ts`, and covers the `version` mismatch
and unparseable-record behaviours from §6.

**No skip hatch when Docker is unreachable.** The containers suite fails loudly.

### AC4 — gating is cosmetic

- A test asserts the package exposes **no enforcement hook** — `hasCapability`
  returns data and nothing in the package can block a call.
- A test asserts a probe receiving `404` yields `degraded`/`not-implemented`
  rather than throwing, which is the client half of "an older service is handled
  gracefully".
- The call path itself is `@paigasus/sdk`'s, and its existing `mapError` tests
  already cover `404` and `UNIMPLEMENTED`. The README cross-references them
  rather than duplicating.
- The README states that `hasCapability` is not a security boundary.

### Test infrastructure

**Do not set the `react-server` condition.** `@paigasus/auth`'s
`vitest.config.ts` records this as MEASURED, and the reason generalises:
`react-server` is also the condition **`react`'s own exports map** switches on,
and that build has no `createContext`. Setting it to satisfy `server-only`
simultaneously breaks `react-dom/client` — which `@testing-library/react`
requires — and anything reaching `next/navigation`. There is no single flat
condition list that satisfies both.

The repo's answer is a permanent **alias**, not a condition:
`resolve.alias['server-only']` points at an empty stub module, so every test in
the package resolves it silently with nothing to remember per file. Conditions
stay `['node', 'import', 'default']` — additive, because dropping
`import`/`default` breaks source-exports `.ts` resolution for every
`@paigasus/*` package.

**Two vitest configs, not three:**

| Config | Covers |
|---|---|
| `vitest.config.ts` | everything except containers; `environment: 'node'`, `include` both `.test.ts` and `.test.tsx`, `exclude` `tests/containers/**` |
| `vitest.containers.config.ts` | real Redis, `test-e2e` only |

A separate config for containers rather than a CLI path filter: vitest applies a
positional path **after** `exclude`, so
`vitest run --config vitest.config.ts tests/containers/` matches zero files and
passes vacuously.

The rendering tests get jsdom through a **per-file docblock**,
`// @vitest-environment jsdom`, which is how `tests/client.test.tsx` does it in
`@paigasus/auth`. The jsdom files set `globals: true` behaviour through a setup
file mirroring `@paigasus/ui`'s: React Testing Library registers its automatic
cleanup only when a global `afterEach` exists, and without it the DOM
accumulates between tests and produces duplicate-id failures belonging to an
earlier test.

`<Capability>` is an async server component, so it is tested by **awaiting the
element it returns** and rendering that — `await Capability({ need, children })`
then `render(element)` — never by calling `render()` on the component itself.

Both configs set `resolve.conditions` **and** `ssr.resolve.conditions`: vitest 5
resolves a node-environment test's imports through the latter.

The probe is faked by injecting a `fetch`-shaped function, never by patching
globals.

---

## 12. Repo gate obligations

Missing any of these reds CI on the implementation PR.

1. **Re-baseline `contracts->proto`.** It is a **project** case using
   `--downstream deep` (`run.sh:34`), its expected set already contains
   `paigasus-sdk-ts` (`run.sh:268-269`), and a `paigasus-discovery-ts` declaring
   `dependsOn: ['paigasus-proto-ts']` lands in it. Revision 1 missed this case
   entirely.

2. **Re-baseline `proto->sdk` and `proto-iam->sdk`.** Both are strict-equality
   task cases whose sets grow by `paigasus-discovery-ts:{build,test}` once this
   package lists `/ts/packages/paigasus-proto/src/**/*` in its `inputs`.

3. **A two-anchor case pair for the new package**, following the
   `ui->console` / `ui-components->console` precedent — one case alone lets an
   `inputs` glob be narrowed to the other subtree while staying green. Anchor one
   on `src/core/` and one on `src/adapters/`. The expected sets must include
   `test-e2e`, as `auth->auth-tasks` does.

4. **Correct the stale `test-e2e` uniqueness comment** at `run.sh:89-91`, which
   records as MEASURED that "`paigasus-auth-ts` is the only project declaring
   `test-e2e`". This package falsifies it. No existing case's observed set
   changes, but the recorded justification would mislead the next case author.
   `ci.yml:185`'s Playwright step comment "(for `@paigasus/auth`'s test-e2e)"
   needs the same correction.

5. **An ESLint boundary block plus its `BOUNDARY_SCOPES` entry.**
   `tests/boundaries.test.ts:191-200` only pairs *declared* scopes with rules, so
   a package with neither is invisible to it. The block bans `next/*` from
   `./types` and records the §2.1 app-shell rule.

6. **No `T`-array or CLAUDE.md marker edit.** `:test-e2e` is already in both, so
   the new task joins an existing target.

7. **Expected sets are derived, not typed** — produced with the same no-flag
   `moon query tasks --affected` traversal `_assert_task_case_impl` uses. This
   applies to **all** of items 1–3, including the two cases revision 1 did not
   list.

8. **`moon.yml` completeness.** `build`, `test`, `typecheck` **and** `test-e2e`.
   `typecheck` needs the proto inputs too (`paigasus-sdk/moon.yml:21-23` is the
   model). `test-e2e` needs `options: cache: false` and must list both vitest
   configs (`paigasus-auth/moon.yml:40-54`). Tasks **append** to the inherited
   definitions in `.moon/tasks/typescript-project.yml`; they never use
   `options.merge: replace`, which would silently drop `@group(sources)`,
   `tsconfig.json`, `package.json`, `/ts/tsconfig.base.json` and
   `/ts/pnpm-lock.yaml` — the defect SMA-503 fixed on `paigasus-console-ts:build`.

9. **`ts:fmt` is a separate whole-tree Prettier gate.** Run it after the last
   TypeScript edit.

No `pnpm-workspace.yaml` edit is needed — it globs `packages/*`. No catalog
additions are needed: `redis`, `testcontainers`, `zod`, `server-only`, `jsdom`
and the four `@testing-library/*` entries are all already pinned. That also
avoids pnpm 11's 24-hour `minimumReleaseAge`, which reds CI on a same-day npm
release.

---

## 13. Decisions rejected

| Rejected | Why |
|---|---|
| Put discovery in `@paigasus/sdk` | The SDK would gain a `redis` dependency and a caching subsystem, making it infrastructure rather than typed clients over proto. |
| Take a `@paigasus/sdk` **dependency** | Its `Presentation` union cannot express these reasons (F8); the probe is a bare `fetch`; and app-shell is banned from reaching the SDK, which a transitive edge would violate. |
| Put discovery in `@paigasus/auth` | Service topology is not identity. |
| Split across sdk, a new package, and ui | Forces `<Capability>` to take a resolved `state` prop rather than the `need=` prop the issue specifies. |
| gRPC for IAM, HTTP for the gateway | Two code paths where one works. ADR-0020 A5 chose HTTP on both. |
| **Report `available` when the last probe FAILED but a stale descriptor is servable** | Hides an outage for the whole stale window. Note this is narrower than revision 1 stated: a stale record whose last outcome was `ok` **does** map to `available` (§6.3), which is what stale-while-revalidate means. |
| A failed probe drops the capability list | Defeats AC2: a blipping service collapses the navigation instead of showing known-but-disabled items. |
| Return `degraded` immediately on a cold cache | The first page load after a deploy would show every service as down — itself a false ops signal. |
| Block until the probe settles, with no deadline | Cold render latency becomes the slowest service's timeout, and several unreachable services add up. |
| `inert` on the disabled wrapper | Removes the node from the accessibility tree. |
| Tailwind utility classes in the wrapper | Silently dropped in production builds without an `@source` line in every consumer. |
| A React context plus `useCapability()` | Forces children into client components, defeating server resolution. |
| Caching a `401`/`403` outcome | One user's expired cookie disables navigation for the whole deployment (§7.2). |

---

## 14. Challenge log

The adversarial pass returned **NEEDS REWORK** with 4 blockers. All were
verified against the code before folding in. Accepted in full:

| Finding | Change |
|---|---|
| BLOCKER — §10's reasons cannot come from `mapError` | Dropped the `@paigasus/sdk` dependency; this package owns the vocabulary (F8, §10) |
| BLOCKER — invariant 5, the fenced write, was dropped | Added `rev` + Lua CAS + the started-before rule (§8.3) |
| BLOCKER — the record→state mapping was undefined and "fresh" ambiguous | Added `isFresh` on `outcomeAt` and the `toState` table (§6.2, §6.3) |
| BLOCKER — a shared entry is poisoned by one user's bad token | `401`/`403` are caller-scoped, never cached (§7.2) |
| MAJOR — lock-wait bound equalled the probe timeout | Separate `LOCK_WAIT_MS`, asserted ordering, final re-read (§5.2, §8.2) |
| MAJOR — Tailwind classes vanish in production | Data attributes and inline styles instead (§9.6) |
| MAJOR — `tabindex` is not inherited | The wrapper is a client component with capture handlers (§9.6) |
| MAJOR — async server component cannot be rendered by testing-library | Three vitest projects, `server-only` stub, await the element (§11) |
| MAJOR — AC3 could not observe the invariant it claimed | Added the never-settling and cross-process cases (§11) |
| MAJOR — AC2 was a flaky wall-clock assertion | Never-settling deferred plus identity assertion (§11) |
| MAJOR — no arm for a failing cache | `cache-unavailable`, plus stated client preconditions (§4.3, §8, §10) |
| MAJOR — `PAIGASUS_SERVICES` keys unvalidated | Validated against the registry slugs; fails construction (§5.1) |
| MAJOR — §12 missed `contracts->proto` and the sdk edge | §12 items 1 and 7; the sdk edge disappears with the dependency |
| MAJOR — no logger, no observability | `ports/logger.ts` with named events (§4) |
| MAJOR — no request-scoped memoization | §9.5, and the latency bound is now conditional |
| MAJOR — no schema version on the record | `version: 1`, mismatch → delete (§6) |
| MAJOR — SSRF surface | `redirect: 'error'`, scheme allowlist, content-type check, size cap (§5.1, §7) |
| MAJOR — `createDiscovery` versus free functions unresolved | Settled on the handle (§9.1) |
| MINOR ×10 | F3 restated; F7 added; `pgs:` prefix; `CapabilityKey`; `hasCapability` doc rule; `test-e2e` comment; boundary block; `describeIssues` note; `SERVICE_STATES` / README / `moon.yml` completeness; invariant 4's try/catch |

**Nothing was rejected.** Every finding was reproducible against the cited code.

The challenge also raised five questions, answered inline: background lock
contention returns immediately (§8.1); `waitUntil` is optional on the container
deploy target and the README covers the freeze-at-response case (§8.1);
`ServiceState` crosses the RSC boundary as plain data, which is why
`ServiceDescriptor` replaced the protobuf message and `readonly string[]`
replaced `ReadonlySet` (§7.1, §9.2); the cross-process Redis case **is** in scope
(§11 AC3 case 3); and the timing orderings are asserted at construction (§5.2).
