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

```ts
import { createDiscovery, createRedisDescriptorCache } from '@paigasus/discovery/server';

const discovery = createDiscovery({
  services: config.PAIGASUS_SERVICES,
  cache: createRedisDescriptorCache(redisClient),
  waitUntil: after, // from 'next/server'
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
app-shell client-reachable. App-shell exports navigation _presentation_ taking
resolved state as props; the **app** composes `<Capability>` around it.
