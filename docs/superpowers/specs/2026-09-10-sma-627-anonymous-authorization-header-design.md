<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-627 — `@paigasus/sdk`: a caller-supplied `authorization` header is refused

**Status:** revision 1
**Issue:** [SMA-627](https://linear.app/smaschek/issue/SMA-627)
**Baseline:** `main` at `1865cf58`. Dependency versions: `@connectrpc/connect` 2.2.0,
`@connectrpc/connect-node` 2.2.0.
**Related:** SMA-508 (the package, merged as PR #229), SMA-625 (error mapping and the chat
client, merged as PR #232).

## 1. Context

SMA-508 deferred a design trade-off rather than deciding it unilaterally. `authInterceptor`
(`ts/packages/paigasus-sdk/src/transport.ts:49-64`) attaches a header only on the `bearer` branch.
On the `{ anonymous: true }` branch it does nothing — it neither sets nor deletes. A caller who
passes their own header therefore sends a credential from a client explicitly declared anonymous:

```ts
const client = createIamClient(TenancyService, { baseUrl }, { anonymous: true });
await client.getOrganization({}, { headers: { authorization: 'Bearer …' } });
```

The package is inconsistent with itself about a caller reaching around its auth binding. A
caller-supplied `CallOptions.contextValues` **throws** (`src/iam.ts:39-49`). A caller-supplied
`CallOptions.headers.authorization` is honoured in silence.

### 1.1 The inconsistency is wider than the issue states

The issue's three options address the anonymous arm only. The bearer arm has the mirror defect:
`req.header.set('authorization', …)` **overwrites** a caller-supplied header without a word. So
there are two silent behaviours, not one:

| `Auth` | caller passes `headers.authorization` | behaviour on `1865cf58` |
| --- | --- | --- |
| `{ bearer }` | yes | silently **overwritten** by the SDK's token |
| `{ anonymous: true }` | yes | silently **forwarded** |

A rule that fixes one row and not the other leaves the package inconsistent, just differently.
This spec fixes both.

### 1.2 One note in the issue is stale

The issue says SMA-625 "is the next issue to touch this surface, so deciding before it lands
avoids a second pass". SMA-625 has landed (`523600b0`, PR #232). That does not weaken the case;
it strengthens it. `createChatClient` took the **narrower** type — `auth: { readonly bearer: string }`,
with no `anonymous` arm at all, because no path through the gateway's `chat_completions` accepts
an anonymous caller. The package's newer surface already treats an unusable auth shape as a
compile error rather than a runtime possibility.

### 1.3 Nothing consumes the package yet

`grep -rn '@paigasus/sdk' ts --exclude-dir=node_modules` returns the package's own files,
`ts/README.md`, `ts/pnpm-workspace.yaml`, the package's `.releaserc.json`, and
`ts/packages/paigasus-next-config/tests/boundaries.test.ts:64-120`. That last file holds **lint-rule
fixture strings**, not imports — it feeds `import { x } from '@paigasus/sdk';` to an eslint
`no-restricted-imports` rule as text. So there is no consumer, and this behaviour change breaks
nobody.

## 2. Measurements

Four facts this design rests on. Each was read from the pinned dependency source in
`ts/node_modules/.pnpm/`, not assumed.

### M1 — the interceptor really does see caller headers

`@connectrpc/connect/dist/cjs/protocol-grpc/request-header.js:25-41`:

```js
function requestHeader(useBinaryFormat, timeoutMs, userProvidedHeaders) {
    const result = new Headers(userProvidedHeaders ?? {});
```

`protocol-grpc/transport.js:76` passes that result as the request's `header`, and `runUnaryCall`
applies the interceptors to that request. So `CallOptions.headers` is the **base** of `req.header`,
and the interceptor sees whatever the caller sent. The refusal can therefore live in the
interceptor and needs no `HeadersInit` handling of its own.

### M2 — casing is not a separate edge case

`req.header` is a `Headers`. Per the Fetch specification `Headers.has` matches case-insensitively,
so `Authorization`, `AUTHORIZATION` and `authorization` are one name. § 5 tests the casings anyway
rather than trusting this, because a fixture that only ever uses the implementer's own spelling
proves nothing about the other spellings.

### M3 — an interceptor throw reaches the caller as a `ConnectError`

`@connectrpc/connect/dist/cjs/protocol/run-call.js`, `runUnaryCall`:

```js
return next(req).then((res) => { done(); return res; }, abort);
```

and `setupSignal`'s `abort`:

```js
const e = controller.signal.aborted
    ? ConnectError.from(getAbortSignalReason(controller.signal), Code.Canceled)
    : ConnectError.from(reason);
```

So through a real transport a plain `Error` thrown by an interceptor surfaces as a `ConnectError`
with `Code.Unknown`, its message prefixed `[unknown] `.

This is **not new** and **not a cost of this design**: the package's existing empty-bearer refusal
throws from the same site and already has exactly this fate. It does differ from the
`contextValues` refusal, which throws synchronously out of `bindAuth`'s proxy and is never wrapped.
§ 4 records that asymmetry so a later reader does not "harmonise" it by accident.

### M4 — a present-but-empty header still registers

`new Headers({ authorization: '' })` yields a `Headers` whose `has('authorization')` is `true` and
whose `get('authorization')` is `''`. An empty caller header is therefore caught by the same test
as a populated one, with no special case.

## 3. The decision

**The SDK owns the `authorization` header. A request that arrives at `authInterceptor` already
carrying one is refused, on both `Auth` arms.**

This is the issue's option 3, generalised from the anonymous arm to both. Three reasons.

**Consistency with the package's own precedent.** `contextValues` is refused rather than merged or
dropped, and `src/iam.ts:44-46` gives the reason: merging would write the bearer into an object the
caller holds, and dropping silently is worse still. A caller-supplied `authorization` header is the
same act — reaching around the SDK's auth binding — and gets the same answer.

**It makes `{ anonymous: true }` mean what SMA-508 § 7.4 says it means.** That section makes `Auth`
a union rather than an optional "so that an unauthenticated call … is a written decision rather
than an omission". A request carrying a credential contradicts a written decision not to send one.

**It closes the bearer row too.** Silently overwriting a caller's header is the same class of
surprise as silently forwarding one, and no separate rule is needed to fix it — refusing on arrival
covers both.

### 3.1 What this rejects, and why

**Option 1, leave it.** The behaviour is defensible in isolation but not alongside the
`contextValues` refusal. With no consumer yet, the cost of deciding is at its lowest it will ever
be, and § 1.3 shows nothing breaks.

**Option 2, delete on the anonymous branch.** A silent drop is the failure mode this package
already rejected once. Its stated risk — dropping a deliberate non-Bearer scheme, such as a proxy
credential — is also answered by § 3.2 rather than by keeping the drop.

**Enforcing in `bindAuth`'s proxy instead of the interceptor.** A proxy check would throw
*synchronously*, which is a real advantage (M3). It is outweighed: the proxy covers only clients
built through `bindAuth`/`createIamClient`, and a caller who writes
`createClient(service, getTransport(opts))` bypasses it entirely. The package's own test suite
builds clients that way (`tests/iam.test.ts:103`). The interceptor sits on the transport and cannot
be routed around.

**Enforcing in both places.** Two error messages and two test blocks for one rule, when the
interceptor alone is sufficient. Rejected on YAGNI grounds.

### 3.2 Exactly one header is claimed

`authorization` and nothing else. The SDK writes exactly one header, so it claims exactly one.

`proxy-authorization` is **deliberately untouched**. It is the correct field for a credential aimed
at an intermediary rather than at the origin server, which is precisely the case option 2's stated
risk describes. A caller who needs to authenticate to a proxy therefore keeps a path, and does not
need this SDK's cooperation to use it. § 5 tests that it survives.

`cookie` and any custom credential-bearing header are likewise untouched. Claiming them would be a
different and much broader rule, and nothing in this repo needs it.

### 3.3 An empty value is refused too

`{ headers: { authorization: '' } }` throws, by M4, with no special case in the code. This is
deliberate, not an accident of the implementation. It is still the caller reaching around the
binding, and refusing it matches the package's existing refusal of an empty bearer — which exists
because an unset environment variable read into a credential should fail loudly and locally rather
than produce a confusing failure on the far end of the call.

### 3.4 Ordering against the empty-bearer refusal

The header check runs **first**, unconditionally, before `req.contextValues.get(authContextKey)`.

Two consequences, both intended. A request that is wrong in both ways — a caller header *and* an
empty bearer — reports the header, deterministically, rather than depending on evaluation order
nobody wrote down. And the check runs before `req.header.set` on the bearer branch, so it can never
trip over the header the SDK itself just wrote.

## 4. The change

One file: `ts/packages/paigasus-sdk/src/transport.ts`. The check is added at the top of
`authInterceptor`, ahead of the existing body, which is otherwise unmodified:

```ts
export const authInterceptor: Interceptor = (next) => async (req) => {
  if (req.header.has('authorization')) {
    throw new Error('@paigasus/sdk: refusing a caller-supplied `authorization` header. …');
  }
  const auth = req.contextValues.get(authContextKey);
  // … unchanged …
};
```

The error message must name the cause and both remedies: pass the credential as `{ bearer }` to the
client factory, or use `{ anonymous: true }` for an unauthenticated call. It should also name
`proxy-authorization` as the untouched field, so a caller with a genuine intermediary credential
reads the way out of the error rather than filing a bug.

A comment at the check records what § 3 decides: that the rule applies to both arms on purpose,
that it precedes the bearer `set` on purpose, and — per M3 — that this refusal reaches the caller
wrapped as a `ConnectError` while the `contextValues` refusal in `src/iam.ts` does not, because the
two throw from different places. That asymmetry is a consequence of where each check must live, not
an oversight to be tidied away.

No public type changes. `Auth`, `TransportOptions`, `createIamClient` and `bindAuth` keep their
signatures. The transport cache, its key, and `disposeTransports` are untouched.

## 5. Tests

All in `ts/packages/paigasus-sdk/tests/transport.test.ts`, beside the existing empty-bearer
refusals, because that file is where interceptor behaviour is already proven and the enforcement
site is the interceptor. The existing `fakeUnaryRequest` / `noopNext` helpers carry the tests; only
a seeded header is new.

| # | Case | Expected |
| --- | --- | --- |
| 1 | `{ anonymous: true }` + `authorization` header | rejects, message names `authorization` |
| 2 | `{ bearer: 'tok' }` + `authorization` header | rejects — the § 1.1 bearer row |
| 3 | header spelled `Authorization` | rejects (M2) |
| 4 | header spelled `AUTHORIZATION` | rejects (M2) |
| 5 | `authorization: ''` | rejects (M4, § 3.3) |
| 6 | `proxy-authorization` only, `{ anonymous: true }` | resolves; the header reaches `next` unchanged; no `authorization` is added |
| 7 | `proxy-authorization` only, `{ bearer: 'tok' }` | resolves; `authorization` is `Bearer tok` **and** `proxy-authorization` survives |
| 8 | `authorization` header + `{ bearer: '' }` | rejects naming the **header**, not the empty bearer (§ 3.4) |

Cases 6 and 7 are the ones that would catch an over-broad implementation — a check written against
a substring or a regex rather than an exact field name would fail them. Case 8 is the one that
would catch a check placed after the `contextValues.get` branch.

The existing tests in that file — a bearer request with no caller header still gets
`Bearer <token>`, an anonymous one still gets none — stay as they are and serve as the regression
guard that the new check does not fire on a clean request.

### 5.1 One seam stays untested, stated rather than hidden

The step from `CallOptions.headers` to `req.header` is Connect's, not this package's. M1 verifies it
by reading `request-header.js:25-41`; **no test in this repo exercises it.** The recording transport
in `tests/iam.test.ts` captures `ContextValues` and never builds a real request header, so an
`iam.test.ts` test seeding a header would prove only that the helper passes a `Headers` to the
interceptor — which § 5's cases already prove directly, with less indirection.

The consequence is bounded and worth naming: if a future `@connectrpc/connect` stopped seeding
`req.header` from `CallOptions.headers`, every test here would still pass while the rule stopped
applying to the path a caller actually uses. That is a dependency-behaviour change, and the honest
answer is that this spec relies on M1 holding, not that a test proves it. Re-read
`request-header.js` on a connect major bump.

## 6. Documentation

Two edits beyond the spec file itself.

A forward reference in `docs/superpowers/specs/2026-09-09-sma-508-sdk-design.md` § 7.4, pointing at
this spec, so a reader of the older design does not conclude that its description of the anonymous
arm is still the whole contract.

The code comment described in § 4.

**No CLAUDE.md entry.** The repo's CLAUDE.md carries repo-wide gotchas — build gates, toolchain
traps, cross-cutting invariants. A single package's public-API rule, enforced by that package's own
tests and readable from its own source, is not one.

## 7. Out of scope

**The chat client.** `createChatClient` builds its own `headers` object literal (`src/chat.ts`) and
accepts none from the caller — its per-call options are `{ signal }` only. It also takes
`{ bearer: string }` with no anonymous arm. There is nothing for this rule to reach.

**Widening the rule to other headers.** § 3.2.

**The `contextValues` refusal.** Unchanged, in `src/iam.ts`, and it stays synchronous.

## 8. Acceptance criteria

1. `authInterceptor` throws when `req.header` already carries an `authorization` header, on both
   `Auth` arms, before it reads the bound `Auth`.
2. The thrown message names `authorization`, names both remedies (`{ bearer }` / `{ anonymous: true }`),
   and names `proxy-authorization` as untouched.
3. `proxy-authorization` and every other header pass through unchanged, and a bearer request that
   carries `proxy-authorization` still receives its `Bearer` header.
4. A request carrying no `authorization` header behaves exactly as it does on `1865cf58`.
5. The eight cases in § 5 are tests in `tests/transport.test.ts` and pass.
6. `moon ci` is green over the repository's full target list, `ts:fmt` included.
