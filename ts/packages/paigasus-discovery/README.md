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

| State     | Condition                  | Render                                   |
| --------- | -------------------------- | ---------------------------------------- |
| absent    | not in `PAIGASUS_SERVICES` | nothing                                  |
| available | configured and answering   | normal                                   |
| degraded  | configured but unreachable | **disabled with a reason, never hidden** |

Hiding a deployed-but-down service turns an outage into an apparent
configuration change and destroys the operator's signal.

Note that `hasCapability()` and `<Capability>` deliberately **disagree** for a
degraded service: `hasCapability()` returns `false`, because the feature must not
be invoked, while `<Capability>` still renders the item disabled rather than
hidden. Use `hasCapability()` for non-UI callers; use `<Capability>` to render.

## Configuration

| Variable                              | Default      | Meaning                                                                                                                                                                                     |
| ------------------------------------- | ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `PAIGASUS_SERVICES`                   | _(required)_ | JSON object, service name to base URL: `{"iam":"http://iam:8080"}`. Keys must be known service slugs; values must be absolute `http:`/`https:` URLs with no credentials, query or fragment. |
| `PAIGASUS_DISCOVERY_NEGATIVE_MS`      | `10000`      | how long a failed probe is honoured                                                                                                                                                         |
| `PAIGASUS_DISCOVERY_FRESH_MS`         | `60000`      | how long a successful probe is honoured                                                                                                                                                     |
| `PAIGASUS_DISCOVERY_STALE_MS`         | `600000`     | the Redis hard TTL; the stale window                                                                                                                                                        |
| `PAIGASUS_DISCOVERY_PROBE_TIMEOUT_MS` | `1500`       | per-probe network deadline                                                                                                                                                                  |
| `PAIGASUS_DISCOVERY_LOCK_WAIT_MS`     | `2500`       | how long a cold loser waits                                                                                                                                                                 |
| `PAIGASUS_DISCOVERY_LOCK_TTL_MS`      | `5000`       | single-flight lock lifetime                                                                                                                                                                 |

`NEGATIVE_MS < FRESH_MS < STALE_MS` and
`PROBE_TIMEOUT_MS < LOCK_WAIT_MS < LOCK_TTL_MS` are asserted at construction.

`@paigasus/next-config`'s `describeIssues` renders a full message only for keys
it owns itself, so a malformed value surfaces as `PAIGASUS_SERVICES: custom`.
The table above is the authoritative statement of each variable's form.

## Usage

Build the handle **inside a request-scoped function**, never at module scope.
The example below is shaped that way on purpose: a bare module-level
`const discovery = createDiscovery({...})` reads as a singleton, and it is
wrong. `getServiceState`'s memo map never removes an entry, because the handle
is meant to die with the request; a process-wide handle would instead replay
the first caller's outcome forever — including a rejected token — for every
later request.

```ts
import { createDiscovery, createRedisDescriptorCache, timingsFromEnv, type Discovery } from '@paigasus/discovery/server';

export function getDiscovery(): Discovery {
  return createDiscovery({
    services: config.PAIGASUS_SERVICES,
    cache: createRedisDescriptorCache(redisClient),
    timings: timingsFromEnv(config), // applies the six PAIGASUS_DISCOVERY_* overrides below
    waitUntil: after, // from 'next/server'
  });
}
```

Call `getDiscovery()` once per request, for example at the top of a server
component, and pass the returned handle down. Build **one handle per
request**: `getServiceState` memoizes per handle, so N `<Capability>` elements
over one service cost one resolution.

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

### Rendering with `<Capability>`

```tsx
import { Capability } from '@paigasus/discovery/react';

<Capability discovery={discovery} need="iam.audit" token={token}>
  <NavLink href="/audit">Audit log</NavLink>
</Capability>;
```

`<Capability>` renders nothing when the service is absent, the children when the
service answers and reports the key, and nothing when the service answers but
lacks the key. When the service is degraded, it renders the children **disabled
with a reason, never hidden**.

Pass a `degraded` render prop to replace the default disabled wrapper with your
own — for example a nav item that renders its own tooltip:

```tsx
<Capability discovery={discovery} need="iam.audit" token={token} degraded={(reason) => <NavItem disabled reason={reason} />}>
  <NavLink href="/audit">Audit log</NavLink>
</Capability>
```

#### Styling the disabled state

The default wrapper carries **no Tailwind utility classes**. Style the disabled
state against these two data attributes instead:

| Attribute                | Value              | Meaning                                                                                                         |
| ------------------------ | ------------------ | --------------------------------------------------------------------------------------------------------------- |
| `data-capability-state`  | `"degraded"`       | the service is configured but not answering right now                                                           |
| `data-capability-reason` | a `DegradedReason` | `timeout`, `network`, `unauthorized`, `not-implemented`, `bad-response`, `server-error`, or `cache-unavailable` |

These two attributes are the **entire styling contract** the default wrapper
exposes.

### Consuming from `@paigasus/app-shell`

Don't. `<Capability>` is an async server component and this package's `./react`
and `./server` entries are `server-only`. `paigasus/boundaries/app-shell` keeps
app-shell client-reachable. App-shell exports navigation _presentation_ taking
resolved state as props; the **app** composes `<Capability>` around it.
