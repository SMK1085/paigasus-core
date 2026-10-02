# SMA-640: Strip `Connection`-nominated headers in the e2e TLS terminator and the counting forwarder

- **Linear:** [SMA-640](https://linear.app/smaschek/issue/SMA-640)
- **Branch:** `feature/sma-640-connection-nominated-headers`
- **Related:** SMA-512 (PR #248, where CodeRabbit raised the finding and the author declined it),
  SMA-641 (`ts/tooling/dev-stack.ts`, which added a third copy of the same filter).
- **Status:** APPROVED by Sven on 2026-10-01. Draft from an unattended Stage 1 run on
  2026-09-27. Challenged once (verdict APPROVE WITH CHANGES); the justified findings are folded
  in (see the Challenge changelog). The approval answers Q1-Q5 (§11). Every cited path and line
  was re-checked against `origin/main` at `8fff8997` on 2026-10-01 (see "Re-check on
  2026-10-01" at the end).
- **Path:** bounded in code size, but it changes an export of `@paigasus/console-core/testing`,
  so it gets a written spec.

## 1. Problem

Three in-process HTTP proxies in `ts/` filter hop-by-hop headers with a fixed name list. Each
one has its own copy of the same `HOP_BY_HOP` set and the same `forwardable()` function:

| Site | Role | Line (on `8fff8997`) |
|---|---|---|
| `ts/packages/paigasus-console-core/testing/tls-terminator.ts` | HTTPS edge for both zones' e2e tiers and for `dev-stack` | `HOP_BY_HOP` at 25, `forwardable()` at 27 |
| `ts/apps/gateway-console/tests/e2e/support/counting-forwarder.ts` | counts connections to `iam-console` in the two-zone tier | 27, 29 |
| `ts/tooling/dev-stack.ts` | the default-zone proxy of the local dev stack (SMA-641) | 81, 83 |

The set is `connection, keep-alive, proxy-connection, te, trailer, transfer-encoding, upgrade`.
Two defects follow from it:

1. **Nominated headers pass through.** RFC 9110 § 7.6.1 says an intermediary must remove every
   field that the `Connection` header names, as well as `Connection` itself. The current code
   removes `Connection` but forwards the fields it names. A request with
   `Connection: x-internal` and `x-internal: 1` arrives at the upstream with `x-internal: 1`.
   The same is true in the response direction: all three sites call
   `forwardable(upstreamRes.headers)` for the response.
2. **Two proxy-auth names are missing.** `proxy-authenticate` and `proxy-authorization` apply to
   the next hop only. RFC 9110 § 11.7.1 and § 11.7.2 say that they apply to the next outbound
   proxy or client. § 11.7.2 also says "A proxy MAY relay the credentials", so RFC 9110 does
   not make them hop-by-hop. RFC 2616 § 13.5.1 is the list that named them hop-by-hop. These
   proxies have no reason to relay proxy credentials, so removing them is a deliberate policy
   choice, not an RFC 9110 requirement. A later reader must not remove them from the set with
   the argument "RFC 9110 does not list them".

The `tunnel()` upgrade path in `tls-terminator.ts` also calls `forwardable(req.headers)` and then
puts `connection: 'Upgrade'` and `upgrade` back. A nominated field on a WebSocket handshake
therefore also passes through today. The hand-written 101 response in `tunnel()`
(`tls-terminator.ts` lines 159-165) copies every upstream header to the client without any
filter, so a nominated field in the upgrade response also passes through.

### 1.1 Why it was declined on SMA-512, and why it is in scope now

On PR #248 the author declined the finding: the proxies are measurement instruments, and every
participant is started by the same fixture. The issue records the condition that changes this:
"the moment either proxy fronts something outside the fixture". SMA-641 then put the terminator
in front of `next dev` in `ts/tooling/dev-stack.ts`, a developer command, and made a third copy
of the filter. The `dev-stack.ts` comment at lines 75-80 says "Widening this set means widening
that copy too" (the same promise appears in `tls-terminator.ts` lines 21-23). So the fix must
reach all three copies, not the two that the issue names.

## 2. Goals

- G1. Every proxy removes the fields that its `Connection` header names, in both directions,
  on the normal request path and on the upgrade path. On the upgrade path this includes the
  101 response that `tunnel()` writes by hand.
- G2. The fixed set also holds `proxy-authenticate` and `proxy-authorization`.
- G3. One definition of the rule. The three sites use it, so a later change cannot reach only
  one of them.
- G4. Tests that send a real `Connection` header through the terminator and the counting
  forwarder with a `node:http` client, and tests that fail when the new code is deleted.
  `dev-stack.ts` cannot run in a test (its own header says so); a text-pin test pins its call
  sites instead (§6.4).
- G5. A nomination never removes `content-length` (decision D6). The fix must not add a
  request-framing desync.

## 3. Non-goals

- Header filtering in any production code path. No production proxy exists in `ts/`.
- `Keep-Alive` parameters, `TE` negotiation, or trailers. The proxies already drop these names.
- A change to how the proxies set `host` and `x-forwarded-*` (see decision D3).
- The existing desync for a chunked request that has a body and a method that Node does not
  chunk by default (for example a chunked `GET`). `transfer-encoding` is in the fixed set today,
  so this case exists before this change. §6.5 measures it, and §10 records it as residual
  risk. No separate issue is opened for it (Q5, D8).

## 4. Design

### 4.1 Approaches considered

- **A. One shared helper in `@paigasus/console-core/testing` (recommended).** A new module
  `testing/hop-by-hop.ts` holds the set and the function. The terminator imports it with a
  relative path. `testing/index.ts` exports it, and `counting-forwarder.ts` and `dev-stack.ts`
  import it from `@paigasus/console-core/testing`. `dev-stack.ts` already imports other names
  from that subpath (`dev-stack.ts:39`). `counting-forwarder.ts` imports nothing from the
  subpath today (it imports only `node:http` and `node:net`), but its package already depends on
  `@paigasus/console-core` and other gateway-console e2e support files import the subpath
  (`two-zone-harness.ts:34`). So no new package dependency edge appears. The `gateway-console`
  `test` and `test-e2e` tasks already list `/ts/packages/paigasus-console-core/testing/**/*` as
  inputs (`moon.yml` lines 231 and 328), so affectedness is already correct.
- **B. Edit the three copies in lockstep.** The smallest diff, and it keeps SMA-641's
  "copied rather than imported" decision. But the defect in this issue is exactly a copy that
  did not keep up with a rule. A fourth proxy would copy the function again.
- **C. A shared helper plus a test that checks the call sites use it.** A text-pin test is an
  established pattern in this repo (`tests/unit/wasm-gate-wiring.test.ts`,
  `action-names.test.ts`, `bulk-replay-ceiling.test.ts`, and each app's
  `e2e-read-only.test.ts`), so the cost is low. The terminator and the counting forwarder are
  pinned by behaviour tests already, so the text pin is needed only for `dev-stack.ts`.

Recommendation: **A, plus the one part of C that `dev-stack.ts` needs** (the text pin in §6.4).
This reverses the SMA-641 review decision (dev-stack review round 1, finding 2). The reason given
there was that the `./testing` export map "exposes only what the e2e harness needs". That reason
was already not exact: `testing/index.ts` lines 4-5 and 12-13 export `buildDevEnv`, `devWorld` and
`DEV_*`, which exist "only for" `dev-stack.ts`. After this change the e2e harness itself
(`counting-forwarder.ts`) also needs the helper. See open question Q1.

### 4.2 The helper: `ts/packages/paigasus-console-core/testing/hop-by-hop.ts`

```ts
// SPDX-License-Identifier: Apache-2.0
import type { IncomingHttpHeaders } from 'node:http';

/**
 * RFC 9110 § 7.6.1 hop-by-hop fields, plus proxy-authenticate / proxy-authorization. RFC 9110
 * § 11.7 does not call the last two hop-by-hop (a proxy MAY relay credentials); RFC 2616
 * § 13.5.1 did. These proxies never relay proxy credentials, so they are removed by policy.
 */
const HOP_BY_HOP: ReadonlySet<string> = new Set([
  'connection', 'keep-alive', 'proxy-authenticate', 'proxy-authorization',
  'proxy-connection', 'te', 'trailer', 'transfer-encoding', 'upgrade',
]);

/** Names that a Connection nomination never removes (decision D6). */
const NEVER_NOMINATED: ReadonlySet<string> = new Set(['content-length']);

export function forwardableHeaders(headers: IncomingHttpHeaders): IncomingHttpHeaders;
```

Behaviour of `forwardableHeaders(headers)`:

1. Read `headers.connection`. Node gives a string for one or more `Connection` lines: it joins
   repeated lines with `", "` (measured on Node v24.16.0, §6.6). `@types/node` 24.13.6 types
   `IncomingHttpHeaders.connection` as `string | undefined` (`http.d.ts:69`), so Node's types do
   not reach an array. The helper also accepts a `string[]` as defence in depth (for example a
   caller that builds headers by hand). The array branch is not reachable through Node's own
   types, and the unit row for it needs a cast (§6.1 row 5).
2. Split every value on `,`. Remove spaces and tabs at both ends of each token. Change it to
   lower case. Node keeps the sender's case and spaces (measured: `"X-B , close"` arrives as
   is), so both steps are necessary. An empty token needs no special step: no header name is
   empty, so it removes nothing.
3. Remove every name in `NEVER_NOMINATED` from the nominated set (decision D6).
4. Copy every entry whose name is in neither the fixed set nor the nominated set, and whose
   value is not `undefined`. Node already lower-cases header names in `req.headers` and
   `res.headers`, so the name comparison is exact.
5. Return a new object. Do not change the input.

A nominated token that is also in the fixed set (`close`, `keep-alive`, `upgrade`) has no extra
effect. `close` names no header, so it removes nothing.

The function is pure. It does not log and it does not throw. That keeps the "never a throw in an
event callback" rule that both proxy files document.

### 4.3 Call sites

| File | Change |
|---|---|
| `testing/tls-terminator.ts` | Delete the local `HOP_BY_HOP` and `forwardable()`. Import `forwardableHeaders` from `./hop-by-hop`. Replace the three calls: request headers in `forward()`, response headers in `forward()`, request headers in `tunnel()`. Filter the hand-written 101 response too (below). Update the doc comment at lines 19-24: there is no copy to keep in step any more. |
| `testing/index.ts` | Add `export { forwardableHeaders } from './hop-by-hop';`. Do not export the set. Update the subpath comment at lines 3-11: the subpath now also holds the hop-by-hop helper that the proxies share. |
| `package.json` (console-core) | Update `_comment_exports`: `./testing` also holds the shared hop-by-hop helper. |
| `gateway-console/tests/e2e/support/counting-forwarder.ts` | Delete the local copy. Import from `@paigasus/console-core/testing`. Replace both calls. |
| `ts/tooling/dev-stack.ts` | Delete the local copy and its "copied rather than imported" comment. Add the name to the existing import at line 39. Replace both calls in `startDefaultZone()` (lines 138-139). In the header comment at line 27, change the stale citation `ts/moon.yml:28-30` to `ts/moon.yml:31-33` (the "No build/typecheck/test here" block moved; see the re-check below). |

`tunnel()` request: keep the spread order. `...forwardableHeaders(req.headers)` comes first, then
the explicit `connection: 'Upgrade'` and `upgrade`. So a handshake with
`Connection: Upgrade, x-foo` reaches the upstream with `Connection: Upgrade`,
`Upgrade: websocket`, and no `x-foo`.

`tunnel()` 101 response: use the same pattern as the request. Build the header lines from
`forwardableHeaders(upstreamRes.headers)`, then add `connection: Upgrade` and
`upgrade: <upstreamRes.headers.upgrade ?? 'websocket'>` back. `Sec-WebSocket-Accept` and the
other handshake fields are not hop-by-hop, so they still reach the browser. The change is about
three lines in the existing `lines` loop.

### 4.4 Error handling and failure modes

The helper is total over every `IncomingHttpHeaders` value. A malformed `Connection` value (only
commas, only spaces) gives an empty nominated set. The proxies keep their current 502 and
socket-destroy paths without change.

A nomination can remove a field that controls the proxy's own behaviour. Three names matter:

- `content-length`: in Node, the outgoing headers object controls the message framing. If a
  nomination removed `content-length` from a `GET`, `HEAD`, `DELETE` or `OPTIONS` request, Node's
  `ClientRequest` would write the body with no framing, and the upstream would read it as a
  second, pipelined request (measured, §6.5). That second request carries the client's own
  `Host` and `X-Forwarded-*`, and it skips the header rewrite in `forward()`. Decision D6
  prevents this: a nomination never removes `content-length`.
- `host`: see decision D3. The removal has no effect in the terminator. In the other two
  proxies Node writes `Host` from the target.
- `transfer-encoding`: the fixed set already removes it, before and after this change. This
  gives the same desync for a chunked request with a body on a method that Node does not chunk by
  default (measured, §6.5). This change does not make it better or worse. §3 and §10 record it.

## 5. Decisions

- **D1. Shared module, not three copies** (approach A, §4.1). It reverses an SMA-641 review
  decision. Sven accepted the new `forwardableHeaders` export on
  `@paigasus/console-core/testing` on 2026-10-01 (Q1).
- **D2. Filter both directions with the same function.** RFC 9110 § 7.6.1 applies to a response
  as much as to a request, and all three sites already use one function for both. This includes
  the 101 response in `tunnel()`.
- **D3. Strict RFC behaviour for `host`.** A request with `Connection: host` loses its `Host`
  field in the helper. In the terminator this has no effect, because `forward()` and `tunnel()`
  set `host` and `x-forwarded-*` after the spread. In the counting forwarder and in
  `dev-stack.ts`, Node's client then writes `Host` from the target (`127.0.0.1:<port>`). No
  client in the fixtures sends this. The spec adds no guard for `host`, because the removal does
  not change message framing. Sven accepted this on 2026-10-01 (Q2).
- **D4. `forwardableHeaders`, not `forwardable`.** The exported name says what the argument is,
  and it does not collide with a local name if a future file keeps its own.
- **D5. Test with `node:http`, not a raw socket, for the normal path.** Measured (§6.6): the
  `node:http` client sends a caller-set `Connection: X-Internal, keep-alive` verbatim, and a
  `node:http` server sends a caller-set response `Connection: x-resp` verbatim. The raw socket is
  needed only for the case of two separate `Connection` lines and for the upgrade handshake.
- **D6. A nomination never removes `content-length`.** This is a deliberate deviation from
  strict RFC 9110 § 7.6.1. Go's `httputil.ReverseProxy` can remove a nominated `Content-Length`
  safely, because Go keeps the body length outside the header map. Node does not: the outgoing
  headers object is the framing. Removing it would add a request-smuggling path (§4.4, §6.5).
  The guard is one set with one name. Sven accepted this on 2026-10-01 (Q2).

- **D7. `ts/tooling/dev-stack.ts` and the terminator's hand-written 101 response are in scope**
  (Q3, accepted by Sven on 2026-10-01). The Linear issue's scope is extended to match (§11 Q3).
- **D8. No separate issue for the chunked-`GET` desync** (Q5, decided by Sven on 2026-10-01). It
  stays a recorded residual risk in §10 only.

## 6. Tests

### 6.1 Unit: `ts/packages/paigasus-console-core/tests/unit/hop-by-hop.test.ts` (new)

Table-driven cases over `forwardableHeaders`:

1. Each of the nine fixed names is removed. Include `proxy-authenticate` and
   `proxy-authorization` by name, so a deletion from the set reds a named row.
2. `connection: 'x-internal'` removes `x-internal` and keeps `x-other`.
3. Case and whitespace: `connection: ' X-Internal ,\tX-Other '` removes both.
4. Joined lines: `connection: 'x-a, X-B , close'` removes `x-a` and `x-b`.
5. An array value (`connection: ['x-a', 'x-b']`) removes both. Node's types do not allow this
   value, and console-core's `tsconfig.json` includes `tests/**`, so the row needs
   `as unknown as IncomingHttpHeaders`. The plan must show the cast.
6. Empty tokens: `connection: ' , ,'` removes nothing else. This row checks that the helper does
   not throw and keeps every other field. It is not a mutation target: no mutation of token
   handling can make it fail, because no header name is empty.
7. `undefined` values are dropped; `set-cookie` arrays pass through unchanged.
8. The input object is not changed.
9. `connection: 'content-length'` keeps `content-length` (decision D6). Also
   `connection: 'Content-Length, x-a'` keeps `content-length` and removes `x-a`.

Per the repo's fixture lesson (fixtures inherit the implementer's assumption), rows 3, 4 and 9
vary case and whitespace on purpose.

### 6.2 Integration: the terminator

Put all terminator cases in console-core's own `tests/unit/`, next to `terminator-upgrade.test.ts`,
where the helper and the terminator are. Do not edit
`ts/apps/iam-console/tests/integration/doubles/tls-terminator.test.ts`: its shared `beforeAll`
upstream (lines 15-32) echoes a fixed set of fields, and its test 1 checks them with `toEqual`,
so adding `headers` to that echo reds test 1.

- New file `ts/packages/paigasus-console-core/tests/unit/terminator-hop-by-hop.test.ts`, with its
  own echo upstream (it echoes `req.headers`) and its own terminator in the `target` form.
  - Request direction: send `Connection: x-internal`, `x-internal: 1`, `x-kept: 1`, and
    `proxy-authorization: Basic x` through `node:https`. Assert that the upstream saw `x-kept`
    and did not see `x-internal` or `proxy-authorization`.
  - Response direction: the upstream answers `Connection: x-resp`, `x-resp: 1`,
    `proxy-authenticate: Basic`. Assert that the client did not receive `x-resp` or
    `proxy-authenticate`.
- `ts/packages/paigasus-console-core/tests/unit/terminator-upgrade.test.ts`: add two cases.
  - Upgrade request: the raw TLS handshake sends `Connection: Upgrade, x-foo` and `x-foo: 1`. The
    upstream's `upgrade` handler records `req.headers`. Assert that `x-foo` is absent and
    `connection` is `Upgrade`.
  - Upgrade response: the upstream answers the 101 with `Connection: Upgrade, x-up` and
    `x-up: 1`. Assert that the client's raw 101 bytes do not hold `x-up`, and still hold
    `Connection: Upgrade`, `Upgrade: websocket` and `Sec-WebSocket-Accept`.

### 6.3 Integration: the counting forwarder

`ts/apps/gateway-console/tests/integration/doubles/counting-forwarder.test.ts` already has a
`get()` helper that takes headers and an echo upstream. Add the same two cases as the §6.2
request and response cases.

### 6.4 Text pin: `dev-stack.ts`

New file `ts/packages/paigasus-console-core/tests/unit/dev-stack-hop-by-hop-wiring.test.ts`. It
reads `ts/tooling/dev-stack.ts` as text and asserts:

1. The file imports `forwardableHeaders` from `@paigasus/console-core/testing`.
2. The file holds `forwardableHeaders(req.headers)` and `forwardableHeaders(upstreamRes.headers)`
   exactly once each.
3. The file does not hold `HOP_BY_HOP` or `function forwardable(`.

Add `'/ts/tooling/dev-stack.ts'` to the `inputs` of `paigasus-console-core-ts:test`
(`ts/packages/paigasus-console-core/moon.yml`), with a comment in the style of the existing
SMA-630 and SMA-661 text-pin inputs. Without it, an edit to `dev-stack.ts` selects no
console-core task and Moon serves a cached pass.

### 6.5 Measurement: a nominated `content-length` (Node v24.16.0)

A probe script (scratchpad only, not in the repo) ran with the pinned
`~/.proto/tools/node/24.16.0/bin/node`. A minimal proxy in the shape of the counting forwarder
removed `connection` and one more named header, then piped the request body to a raw TCP
upstream that recorded the bytes. The client sent `GET /a` with a 38-byte body that is itself
`GET /smuggled HTTP/1.1\r\nHost: evil\r\n\r\n`.

- Baseline, `content-length` kept: the upstream got `content-length: 38` and then the body. One
  request.
- `content-length` removed (what a strict nomination would do): the upstream got
  `GET /a HTTP/1.1\r\nhost: x\r\nConnection: keep-alive\r\n\r\nGET /smuggled HTTP/1.1\r\nHost: evil\r\n\r\n`.
  No framing header, so the body reads as a second request.
- Chunked `GET`, `transfer-encoding` removed (the fixed set, before and after this change): the
  upstream got the same bytes as the case above. Node de-chunked the body and wrote it with no
  framing.

Note: the proto shim in `ts/` resolved `node` to v24.18.1 in this session, not the `.prototools`
pin 24.16.0. The measurement above used the 24.16.0 binary directly. v24.18.1 gave identical
bytes.

### 6.6 Measurements taken for this spec (header handling)

A probe script (scratchpad only, not in the repo) on Node v24.16.0:

- A `node:http` client with `headers: { connection: 'X-Internal, keep-alive', 'x-internal': 'secret' }`:
  the server saw `connection: 'X-Internal, keep-alive'` and `x-internal: 'secret'`.
- A server that sets `connection: 'x-resp'` and `x-resp: '1'`: the client saw both.
- Raw socket, two lines `Connection: x-a` and `Connection: X-B , close`: the server saw
  `connection: 'x-a, X-B , close'`.

### 6.7 Proof that the tests bite

The implementer runs each of these mutations, one at a time, and records that at least one test
goes red for each. Mutations must compile (`tsc` must pass), per the repo's lesson that a
mutation that fails to compile proves nothing. The mutations are described by behaviour, not by
a method name, so that an implementation with a regex split still has a real mutation for each.

| # | Mutation | Expected red |
|---|---|---|
| M1 | Remove the nominated-token step from `forwardableHeaders` | §6.1 rows 2-5, and each nominated-field case in §6.2 and §6.3 |
| M2 | No case fold of the nominated tokens | §6.1 rows 3 and 4 |
| M3 | No whitespace strip of the nominated tokens | §6.1 rows 3 and 4 |
| M4 | Remove `proxy-authorization` from the set | §6.1 row 1, the request cases in §6.2 and §6.3 |
| M5 | Remove `proxy-authenticate` from the set | §6.1 row 1, the response cases in §6.2 and §6.3 |
| M6 | In `tls-terminator.ts`, pass `req.headers` unfiltered in `forward()` | §6.2 request case |
| M7 | In `tls-terminator.ts`, pass `upstreamRes.headers` unfiltered in `forward()` | §6.2 response case |
| M8 | In `tunnel()`, pass `req.headers` unfiltered | §6.2 upgrade request case |
| M9 | In `counting-forwarder.ts`, pass `req.headers` unfiltered | §6.3 request case |
| M10 | In `counting-forwarder.ts`, pass `upstreamRes.headers` unfiltered (line 66 today) | §6.3 response case |
| M11 | In `tunnel()`, build the 101 lines from `upstreamRes.headers` unfiltered | §6.2 upgrade response case |
| M12 | Remove the `NEVER_NOMINATED` step (a nomination removes `content-length`) | §6.1 row 9 |
| M13 | In `dev-stack.ts`, pass `req.headers` unfiltered | §6.4 assertion 2 |
| M14 | In `dev-stack.ts`, pass `upstreamRes.headers` unfiltered | §6.4 assertion 2 |

M6-M11 pin the production call sites, not only the helper (the "guard-the-guard" lesson). M13
and M14 pin the `dev-stack.ts` call sites by text only; §10 records the limit of a text pin.
§6.1 row 6 is not a mutation target (see §6.1).

## 7. Files

| File | Kind |
|---|---|
| `ts/packages/paigasus-console-core/testing/hop-by-hop.ts` | new |
| `ts/packages/paigasus-console-core/testing/index.ts` | edit (one export, subpath comment) |
| `ts/packages/paigasus-console-core/package.json` | edit (`_comment_exports` text only) |
| `ts/packages/paigasus-console-core/testing/tls-terminator.ts` | edit |
| `ts/packages/paigasus-console-core/moon.yml` | edit (one `test` input: `/ts/tooling/dev-stack.ts`) |
| `ts/packages/paigasus-console-core/tests/unit/hop-by-hop.test.ts` | new |
| `ts/packages/paigasus-console-core/tests/unit/terminator-hop-by-hop.test.ts` | new |
| `ts/packages/paigasus-console-core/tests/unit/dev-stack-hop-by-hop-wiring.test.ts` | new |
| `ts/packages/paigasus-console-core/tests/unit/terminator-upgrade.test.ts` | edit (two cases) |
| `ts/apps/gateway-console/tests/e2e/support/counting-forwarder.ts` | edit |
| `ts/apps/gateway-console/tests/integration/doubles/counting-forwarder.test.ts` | edit (two cases) |
| `ts/tooling/dev-stack.ts` | edit |

`iam-console` and `gateway-console` already list `/ts/packages/paigasus-console-core/testing/**/*`
as inputs, and `ts/moon.yml` lists `tooling/**/*` for lint and fmt. The only `moon.yml` change is
the §6.4 input.

Affected set. The implementer confirms it with `moon query tasks --affected` and parses the JSON
per the root `CLAUDE.md` (one target per `tasks[project][task]`; do not grep `"target"`). The
expected selections include:

- `paigasus-console-core-ts:test`, `paigasus-console-core-ts:typecheck`,
  `paigasus-console-core-ts:build`.
- `paigasus-console-core-ts:test-e2e` (a `tests/**` edit selects it; `cache: false`; needs
  Docker).
- `iam-console-ts:test`, `iam-console-ts:test-e2e` (`ts/apps/iam-console/moon.yml:428`).
- `gateway-console-ts:test`, `gateway-console-ts:test-e2e`
  (`ts/apps/gateway-console/moon.yml:404`).
- `ts:lint`, `ts:fmt`.

Two of the `test-e2e` tiers need Docker. Docker is available to the implementer, so all three
Docker-backed tiers run locally before the push (Q4). The expected result for the e2e tiers is green
with no change: browsers send only `keep-alive` or `Upgrade` tokens in `Connection`, and the
upstream Next servers do not nominate other fields.

## 8. Acceptance criteria

1. AC1. A request through the terminator, the counting forwarder, or the dev-stack default-zone
   proxy does not carry any field that its `Connection` header names, except `content-length`
   (D6). A response in the other direction does not either. This includes the terminator's
   upgrade request and its hand-written 101 response. The terminator and the counting forwarder
   meet AC1 by behaviour tests (§6.2, §6.3). `dev-stack.ts` meets AC1 by construction (it calls
   the shared helper), by review, and by the §6.4 text pin.
2. AC2. `proxy-authenticate` and `proxy-authorization` are not forwarded in either direction.
3. AC3. The upgrade path of the terminator removes nominated fields and still sends
   `Connection: Upgrade` and `Upgrade` to the upstream and to the client. The existing upgrade
   tests stay green.
4. AC4. The hop-by-hop set exists once in `ts/`.
   `grep -rln "'proxy-connection'" ts --include='*.ts' --exclude-dir=node_modules --exclude-dir=.next`
   finds exactly one file: `ts/packages/paigasus-console-core/testing/hop-by-hop.ts`. (Today it
   finds three.) If a test file must spell that literal, change the grep to exclude `tests/`
   and record why.
5. AC5. The tests in §6 exist, pass, and each mutation in §6.7 reds at least one of them.
6. AC6. `ts:fmt`, `ts:lint`, the `typecheck` targets, and the full `ci-targets` command in the
   root `CLAUDE.md` pass. In addition, the local command
   `pnpm --dir ts exec tsc -p tooling/tsconfig.json --noEmit` passes: no CI task type-checks
   `dev-stack.ts` (its header, lines 24-29).
7. AC7. A nomination of `content-length` does not remove it (§6.1 row 9, M12).

## 9. Verification

- `moon run paigasus-console-core-ts:test iam-console-ts:test gateway-console-ts:test`.
- `moon run paigasus-console-core-ts:typecheck iam-console-ts:typecheck gateway-console-ts:typecheck`
  and `ts:fmt`, `ts:lint`.
- `pnpm --dir ts exec tsc -p tooling/tsconfig.json --noEmit` (the only type check that reaches
  `dev-stack.ts`).
- The Docker-backed tiers: `paigasus-console-core-ts:test-e2e`, `iam-console-ts:test-e2e`,
  `gateway-console-ts:test-e2e`. Expected: green with no change (§7).
- The §6.7 mutation table, run whole after the last fix.
- The full `ci-targets` command from the root `CLAUDE.md`. On this Mac, re-run the bash-4+ gates
  with Homebrew bash, per the root `CLAUDE.md`. No `repo:*` gate is expected to change: this is
  `ts/` test and tooling code only.

## 10. Residual risk

- `dev-stack.ts` has no behaviour test. The §6.4 text pin reds a revert of either call site to
  unfiltered headers, and a return of a local copy. A text pin cannot prove runtime behaviour: a
  change that keeps the pinned strings but bypasses them (for example a second, unfiltered
  `httpRequest`) reds nothing.
- No CI task type-checks `dev-stack.ts`. `ts:lint` (`recommendedTypeChecked`) can find a name
  that does not resolve through the `no-unsafe-*` rules, but it does not find a wrong signature.
  Only the local `tsc -p tooling/tsconfig.json` in AC6 finds one. A later PR that skips it can
  break the call site with CI green.
- A chunked request with a body on a method that Node does not chunk by default (for example a
  chunked `GET`) desyncs at the proxy, because `transfer-encoding` is in the fixed set and Node
  then writes the body with no framing (measured, §6.5). This exists before this change and is
  not changed by it. The exposure is low: a browser cannot send such a request through
  `fetch`/XHR, and every listener binds to 127.0.0.1. Sven decided on 2026-10-01 that no
  separate issue is opened for it (Q5, D8).
- D6 deviates from strict RFC 9110 § 7.6.1 for `content-length`. A peer that nominates it on
  purpose still sees it forwarded.

## 11. Open questions

- **Q1.** Approach A reverses the SMA-641 dev-stack review decision (round 1, finding 2) to copy
  the filter rather than import it. This spec argues that the reason for that decision was
  already not exact (the subpath exports `buildDevEnv`, `devWorld` and `DEV_*` only for
  `dev-stack.ts`) and no longer holds (the e2e harness now needs the helper). Does Sven accept
  the new export on `@paigasus/console-core/testing`? If he chooses approach B (three copies in
  lockstep) instead, the test plan changes: §6.1 has no shared helper to unit-test, so the three
  copies each need the §6.1 rows, or the §6.1 table runs against each copy through a
  parameterised test that imports the copies. That B test plan is not written here.
  **Answer (Sven, 2026-10-01): yes.** Add the shared `forwardableHeaders` export on
  `@paigasus/console-core/testing`. This reverses the SMA-641 decision to copy the filter.
  Approach A stands (D1).
- **Q2.** Decision D3 follows the RFC strictly for `host`, so `Connection: host` removes `Host`
  in the counting forwarder and in `dev-stack.ts` (Node then writes `Host` from the target).
  Decision D6 protects `content-length` from nomination, a deliberate deviation from the RFC.
  Does Sven accept this split? The alternatives are: also protect `host`; or strict RFC
  behaviour for `content-length` with the desync written down as a known risk.
  **Answer (Sven, 2026-10-01): accepted as written.** `content-length` is protected from a
  `Connection` nomination (D6). `host` follows strict RFC behaviour (D3).
- **Q3.** The issue's Scope section names only two files (checked on Linear, 2026-09-27). The
  Linear comment from this run records the scope change. This spec also changes `ts/tooling/dev-stack.ts`, because
  SMA-641 made it a third copy and its comment asks for the change in step. Confirm that the
  third file is in scope for SMA-640. If yes, record the scope change on the Linear issue.
  **Answer (Sven, 2026-10-01): yes.** `ts/tooling/dev-stack.ts` and the terminator's hand-written
  101 response are in scope (D7). The Linear issue description has a "Scope extended on
  2026-10-01" section that records this.
- **Q4.** Does the implementer have Docker for the three Docker-backed `test-e2e` tiers that
  this change selects (§7)? If not, CI is the first place those tiers run.
  **Answer (Sven, 2026-10-01): yes.** Docker is available. The implementer runs the three
  Docker-backed `test-e2e` tiers locally (§9).
- **Q5.** Should a separate issue fix the existing chunked-`GET` desync (§10)? One fix is for the
  proxies to answer 400 to a request that has `transfer-encoding` on a method that Node does not
  chunk, or to set `content-length` from a buffered body.
  **Answer (Sven, 2026-10-01): no.** No separate issue. The desync stays a recorded residual
  risk in §10 (D8).

## Challenge changelog

**Round 1 verdict: APPROVE WITH CHANGES** (0 blocker, 3 major, 10 minor, 4 questions). Each
finding was checked against the repo on `4051df5e`.

Folded:

- MAJOR, 101 response contradicted G1/AC1. Verified at `tls-terminator.ts:159-165` (the `lines`
  loop copies every upstream header). Folded: the 101 is now in scope, filtered then
  `connection`/`upgrade` added back (§4.3); removed from §3 and §10; new §6.2 case; M11.
- MAJOR, a nominated `content-length` removes request framing. Measured on Node v24.16.0 (§6.5):
  the body arrives as a second, unframed request. Folded: decision D6 (`NEVER_NOMINATED`), G5,
  §4.4 rewritten, §6.1 row 9, M12, AC7. The existing chunked-`GET` case is measured too and added
  to §3, §10 and Q5.
- MAJOR, no type check reaches `dev-stack.ts`. Verified at `dev-stack.ts:24-29` and
  `ts/moon.yml:26`. Folded: the local `tsc -p tooling/tsconfig.json` is in AC6 and §9; the false
  §10 text is corrected.
- MINOR, array value reason. Verified `@types/node` 24.13.6 `http.d.ts:69`
  (`connection?: string | undefined`). Folded: §4.2 step 1 and §6.1 row 5 (cast).
- MINOR, §6.1 row 6 cannot fail. Folded: the "drop empty tokens" step is removed from §4.2; row 6
  is a no-throw check, not a mutation target.
- MINOR, missing mutation for the counting forwarder response. Folded: M10. M2 and M3 are now
  described by behaviour.
- MINOR, weak AC4 grep. Verified: the `'proxy-connection'` literal has exactly three hits today.
  Folded: AC4 uses it.
- MINOR, §4.1 overstated approach C. Folded: C's text pin is adopted for `dev-stack.ts` only
  (§6.4), with a new `test` input in console-core's `moon.yml`.
- MINOR, §6.2 ambiguous for the iam-console test. Verified the `toEqual` echo at lines 15-32.
  Folded: all terminator cases move to console-core `tests/unit/`; the iam-console test is not
  edited.
- MINOR, G4/AC1 said "each proxy". Folded: G4 and AC1 name how `dev-stack.ts` meets AC1.
- MINOR, RFC citation for the proxy-auth fields. Folded: §1 item 2 and the helper doc comment now
  cite RFC 9110 § 11.7 accurately and RFC 2616 § 13.5.1 as the source of the hop-by-hop list.
- MINOR, placeholder ids and incomplete affected set. Folded: real ids (`iam-console-ts`,
  `gateway-console-ts`), the three `test-e2e` tiers, the Docker need and the expected result
  (§7, §9, Q4).
- MINOR, two comments not updated, and the "both importers already import" claim. Folded: the
  `testing/index.ts` comment and `_comment_exports` are in §4.3 and §7; §4.1 now says that
  `counting-forwarder.ts` imports nothing from the subpath today.
- QUESTION on Q1 (test plan under B, and the stronger argument for A). Folded into §4.1 and Q1.
- QUESTIONS on `content-length`/`host`, Docker, and the Linear scope. Folded into Q2, Q4, Q3.

Rejected: none. One detail was added beyond the critique: the session's proto shim resolved
`node` to v24.18.1, not the pinned 24.16.0, so §6.5 records that the measurement used the 24.16.0
binary directly.

## Re-check on 2026-10-01

Sven approved the spec on 2026-10-01. The spec was written on 2026-09-27, so every cited path,
line and assumption was checked again against `origin/main` at `8fff8997`.

Confirmed with no change:

- `tls-terminator.ts`: doc comment 19-24, `HOP_BY_HOP` 25, `forwardable()` 27, the `forward()`
  calls at 103 and 106, the `tunnel()` call at 148, and the 101 `lines` loop at 159-165.
- `counting-forwarder.ts`: 27, 29, and the calls at 51 and 66. It still imports nothing from
  `@paigasus/console-core/testing`.
- `dev-stack.ts`: header 24-29, import at 39, comment 75-80, `HOP_BY_HOP` 81, `forwardable()`
  83, calls at 138-139.
- `testing/index.ts`: comment 3-11, `buildDevEnv`/`DEV_ZONES` at 12, `devWorld` and `DEV_*`
  descriptors at 13. `two-zone-harness.ts:34` imports the subpath.
- `gateway-console/moon.yml` inputs at 231 and 328, `test-e2e` input at 404.
  `iam-console/moon.yml` `test-e2e` input at 428. `ts/moon.yml:26` is the `tooling/**/*` input.
- `iam-console/tests/integration/doubles/tls-terminator.test.ts` echo upstream at lines 15-32.
- `counting-forwarder.test.ts` has the `get()` helper (line 39) with a `headers` option.
- console-core `moon.yml`: the `test` task inputs hold the SMA-630 and SMA-661 text-pin entries.
- The text-pin tests named in §4.1 exist. `terminator-upgrade.test.ts` exists.
- `@types/node` 24.13.6 `http.d.ts:69` types `connection` as `string | undefined`.
- AC4: the `'proxy-connection'` grep still finds exactly three files.
- `.prototools` pins `node = "24.16.0"`. The `ts/` shim still resolves `node` to v24.18.1, as §6.5
  recorded.

Changed:

- The branch name in the header: `feature/sma-640-connection-nominated-headers` (the actual
  branch).
- The §1 table commit: `83d446fc` became `8fff8997`. The line numbers did not move.
- §4.3: `dev-stack.ts` line 27 cites `ts/moon.yml:28-30` for the "No build/typecheck/test here"
  block. On `8fff8997` that block is at `ts/moon.yml:31-33`. The implementer corrects the
  citation while the file is open. §4.3 also names the call-site lines 138-139.
- The approval answers: Q1-Q5 in §11, D1, D3, D6, the new D7 and D8, §3, §7 and §10.
