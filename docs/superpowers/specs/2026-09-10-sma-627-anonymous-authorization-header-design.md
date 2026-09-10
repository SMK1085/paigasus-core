<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-627 — `@paigasus/sdk`: a caller-supplied `authorization` header is refused

**Status:** revision 2, after the adversarial challenge (2026-09-10)
**Issue:** [SMA-627](https://linear.app/smaschek/issue/SMA-627)
**Baseline:** `main` at `1865cf58`. `pnpm exec vitest run` in `ts/packages/paigasus-sdk` reports
236 tests in 12 files, all pass. Dependency versions: `@connectrpc/connect` 2.2.0,
`@connectrpc/connect-node` 2.2.0.
**Related:** SMA-508 (the package, merged as PR #229), SMA-625 (error mapping and the chat
client, merged as PR #232).

Revision 2 changes the thrown error's type, rewrites two rationales that cited the source
incorrectly, and adds the streaming path. § 9 lists what the challenge moved and why.

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
`req.header.set('authorization', …)` (`src/transport.ts:61`) **overwrites** a caller-supplied header
without a word. So there are two silent behaviours, not one:

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

### 1.3 Nothing consumes the package, and nothing can

The decisive fact is the manifest, not a grep: `ts/packages/paigasus-sdk/package.json:6-7` is
`"version": "0.0.0"` with `"private": true`. The package has never been published, so it has no
external consumer by construction, and this behaviour change can break none.

Inside the repository, `grep -rn '@paigasus/sdk' ts --exclude-dir=node_modules` returns twenty
lines across eleven files. **None is an import of the package.** They fall into three classes:

| Class | Where |
| --- | --- |
| eslint `no-restricted-imports` fixture strings and rule messages | `packages/paigasus-next-config/tests/boundaries.test.ts:64,65,69,106,120,231`; `packages/paigasus-next-config/src/eslint.mjs:18,28,99,103,112,123,228` |
| a package-name list in config | `packages/paigasus-next-config/src/index.ts:12` (`SOURCE_ONLY_PACKAGES`); `ts/README.md`; `ts/pnpm-workspace.yaml`; the package's own `.releaserc.json` |
| prose comments naming it as a future consumer | `packages/paigasus-auth/src/adapters/claims-resolver.ts:8`; `apps/paigasus-console/app/runtime-config.ts:3`; `packages/paigasus-next-config/src/{runtime.ts:124,189}`; `packages/paigasus-proto/src/index.test.ts:6`; `packages/paigasus-next-config/tests/runtime.test.ts:154` |

`packages/paigasus-auth/src/adapters/claims-resolver.ts:8` is the first **planned** consumer — its
comment records that `@paigasus/sdk` is still a stub. § 3.5 takes that consumer's likely call shape
into account when specifying the error message.

## 2. Measurements

Six facts this design rests on. Each was read from the pinned dependency source under
`ts/node_modules/.pnpm/@connectrpc+connect@2.2.0_@bufbuild+protobuf@2.14.1/node_modules/@connectrpc/connect/`.

**Citations are to the `dist/esm/` build throughout.** `paigasus-sdk` is `"type": "module"`
(`package.json:9`), so ESM is the build it loads. The CJS build is byte-equivalent in behaviour but
differs in text — it carries `connect_error_js_1.` style require prefixes — and neither shipped
build contains the `??` and optional-chaining forms of the unshipped TypeScript source. Quote what
ships.

### M1 — the interceptor really does see caller headers

`dist/esm/protocol-grpc/request-header.js:21-22`:

```js
export function requestHeader(useBinaryFormat, timeoutMs, userProvidedHeaders) {
    const result = new Headers(userProvidedHeaders !== null && userProvidedHeaders !== void 0 ? userProvidedHeaders : {});
```

The unary path reaches it through a wrapper, not directly: `dist/esm/protocol-grpc/transport.js:76`
calls `requestHeaderWithCompression(...)`, which calls `requestHeader` at
`request-header.js:48`. The result becomes the request's `header`, and `runUnaryCall` applies the
interceptors to that request.

So `CallOptions.headers` is the **base** of `req.header`, and the interceptor sees whatever the
caller sent. The refusal can therefore read a normalised `Headers` and needs no `HeadersInit`
handling of its own.

### M2 — casing is not a separate edge case

`req.header` is typed `Headers` on both `UnaryRequest` and `StreamRequest`. Per the Fetch
specification `Headers.has` matches case-insensitively, so `Authorization`, `AUTHORIZATION` and
`authorization` are one name. § 5 tests the casings anyway rather than trusting this, because a
fixture that only ever uses the implementer's own spelling proves nothing about the other
spellings. § 5.2 records that these two cases are weak ones.

### M3 — an interceptor throw is wrapped by `ConnectError.from`

`dist/esm/protocol/run-call.js:27-30`, `runUnaryCall`:

```js
return next(req).then((res) => { done(); return res; }, abort);
```

and `setupSignal`'s `abort` at `:106-115`:

```js
const e = controller.signal.aborted
    ? ConnectError.from(getAbortSignalReason(controller.signal), Code.Canceled)
    : ConnectError.from(reason);
```

So a plain `Error` thrown by an interceptor surfaces as a `ConnectError` with the default
`Code.Unknown`. § 3 makes this the reason **not** to throw a plain `Error`.

### M4 — a present-but-empty header still registers

`new Headers({ authorization: '' })` yields a `Headers` whose `has('authorization')` is `true` and
whose `get('authorization')` is `''`. An empty caller header is therefore caught by the same test
as a populated one, with no special case.

### M5 — the streaming path behaves identically

`authInterceptor` is an `Interceptor`, whose `next` is
`AnyFn = (req: UnaryRequest | StreamRequest) => …` (`dist/esm/interceptor.d.ts:37`), and
`createGrpcTransport` installs the same interceptor array on both arms. `protocol-grpc/transport.js:156`
seeds a stream request's `header` from the caller's headers exactly as `:76` does for unary, and
`runStreamingCall` routes a rejection through the same `setupSignal` `abort`. The design therefore
holds unchanged on the streaming path.

No IAM service declares a streaming RPC today — `grep -c 'stream ' contracts/proto/paigasus/iam/v1/iam.proto`
is zero — so the streaming test in § 5 is forward cover, not a live case. It exists because an
implementer worried about streaming might write `if (!req.stream && req.header.has(…))`, and every
unary case would still pass.

### M6 — a thrown `ConnectError` survives with its code intact

`dist/esm/connect-error.js:68-71`:

```js
static from(reason, code = Code.Unknown) {
    if (reason instanceof ConnectError) {
        return reason;
    }
```

An existing `ConnectError` is returned unchanged. And `abort` (M3) overrides the code to
`Code.Canceled` only when `controller.signal.aborted` is already true, which it is not at
interceptor time — the interceptor runs before any abort. So a `ConnectError` thrown from the
interceptor reaches the caller with the code this SDK chose.

## 3. The decision

**The SDK owns the `authorization` header. A request that arrives at `authInterceptor` already
carrying one is refused with a `ConnectError(…, Code.InvalidArgument)`, on both `Auth` arms.**

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

### 3.1 The error is a `ConnectError`, not a plain `Error`

A plain `Error` reaches the caller as `ConnectError` with `Code.Unknown` (M3). That code is
**not in this package's own presentation table**: `src/errors/transport-status.ts:29-41` has no
`Code.Unknown` row, so `presentationForGrpcCode` falls through to `'generic'` at `:80`.

The consequence is that the SDK would render a caller programming error as an unclassified service
failure. A consumer routes every failure through `@paigasus/sdk/errors`, so an operator would read
"the service is broken" when in fact the caller passed a header it should not have. This package
already identified and fixed exactly this defect once, for a different input:
`src/chat.ts:218-220` records that a caller's unserializable request body "was caught by the
transport arm and reported as a gateway `degraded`, blaming the service for the caller's input".

`Code.InvalidArgument` maps to `'invalid-input'` (`transport-status.ts:36`), which is what this is.
M6 shows the code survives the wrapping.

**The existing empty-bearer refusal moves with it.** `src/transport.ts:56-60` throws a plain `Error`
today and therefore has the same `generic` fate. It is the same kind of caller error, thrown three
lines away, and leaving the two inconsistent would recreate in miniature the split this whole spec
exists to close. Its tests assert `.rejects.toThrow(/empty|whitespace/)`, which a `ConnectError`
still satisfies — `ConnectError` preserves the message, prefixing it with `[invalid_argument] `.

The `contextValues` refusal in `src/iam.ts` does **not** move. It throws synchronously from
`bindAuth`'s proxy, before any transport is involved, and never passes through `ConnectError.from`.
Making it a `ConnectError` would dress a synchronous programming error as an RPC outcome.

### 3.2 What this rejects, and why

**Option 1, leave it.** The behaviour is defensible in isolation but not alongside the
`contextValues` refusal. With no consumer and no published version (§ 1.3), the cost of deciding is
the lowest it will ever be.

**Option 2, delete on the anonymous branch.** A silent drop is the failure mode this package
already rejected once. § 3.3 addresses its stated risk directly rather than by keeping the drop.

**Enforcing in `bindAuth`'s proxy instead of the interceptor.** A proxy check would throw
*synchronously*, which is a real advantage: a caller catches it with or without `await`. Two things
decide against it, and neither is the "a caller can bypass it" argument revision 1 made — that
argument was **wrong**. `getTransport` is not on the public surface: `package.json:13-19` maps only
`.`, `./chat`, `./iam`, `./errors` and `./errors/types`, and neither `src/index.ts:12-19` nor
`src/iam.ts:12-14` re-exports it. No external caller can build a client that reaches
`authInterceptor` without going through `bindAuth`. `tests/iam.test.ts:103` does not bypass it
either — it reads `bindAuth(createClient(TenancyService, transport), { anonymous: true })`.

What decides it is, first, that `CallOptions.headers` is a `HeadersInit` union
(`dist/esm/call-options.d.ts:16`), so a `bindAuth` check would have to normalise
`Headers | string[][] | Record<string, string>` by hand, while `req.header` is already a normalised
`Headers` (M1). Second, the interceptor is where this package's auth policy already lives — the
empty-bearer refusal is four lines away — and keeping one policy in one place is worth more than
the synchronous throw. That `bindAuth` happens to cover 100% of today's public surface makes the
interceptor choice defence in depth rather than necessity, and § 7 records what would change if
`getTransport` were ever exported.

**Enforcing in both places.** Two error messages and two test blocks for one rule, when the
interceptor alone is sufficient. Rejected on YAGNI grounds.

### 3.3 Exactly one header is claimed, and `Bearer` becomes the only scheme

`authorization` and nothing else. The SDK writes exactly one header, so it claims exactly one.
`cookie` and any custom credential-bearing header are untouched — claiming them would be a
different and much broader rule, and nothing in this repo needs it.

`proxy-authorization` is **deliberately untouched**. It is the correct field for a credential aimed
at an intermediary rather than at the origin server, so a caller who must authenticate to a proxy
keeps a path and does not need this SDK's cooperation to use it. § 5 tests that it survives.

**That covers half of option 2's stated risk, and the other half has no answer. Say so.** The risk
as the issue states it is "silently drops a deliberate non-Bearer scheme (a proxy or gateway auth
header)". `proxy-authorization` covers the intermediary case. It does not cover a non-`Bearer`
scheme aimed at the **origin**: `Basic` (OAuth's `client_secret_basic`, conventional for the
`AuthnService` this package exports), `DPoP`, or `Negotiate`. `Auth` hard-codes `Bearer`
(`src/transport.ts:61`), so after this change **`Bearer` is the only `Authorization` scheme this SDK
can send to IAM, and a caller needing another has no path at all.**

That is accepted, not overlooked. Every IAM service this package exports is reached with a bearer
token today, and widening `Auth` for a scheme nobody needs would be speculative. The consequence is
written here so that a caller who does need one files an issue to widen `Auth` rather than reading
the refusal as a bug.

### 3.4 An empty value is refused too

`{ headers: { authorization: '' } }` throws, by M4, with no special case in the code. This is
deliberate, not an accident of the implementation. It is still the caller reaching around the
binding, and refusing it matches the package's existing refusal of an empty bearer — which exists
because an unset environment variable read into a credential should fail loudly and locally rather
than produce a confusing failure on the far end of the call.

### 3.5 The error message: three requirements and one prohibition

The message must name **the cause**, and **both remedies** — pass the credential as `{ bearer }` to
the client factory, or use `{ anonymous: true }` for an unauthenticated call — and
**`proxy-authorization`** as the field this client does not touch, so a caller with a genuine
intermediary credential reads the way out of the error rather than filing a bug.

**It must never contain the header's value.** The obvious debugging-friendly implementation is
`` `…: ${req.header.get('authorization')}` ``, which writes a live bearer token into an exception
message, a container log, and any error reporter that catches it. This package already treats
credential retention as a defect — `src/transport.ts:70-76` exists precisely to stop a `bearer`
reaching the transport cache key and being retained for the process's lifetime. § 5's case 11 makes
the prohibition testable, and AC 2 carries it, because a requirement stated only in prose is one
nobody can fail.

**One caller shape is worth naming in the message.** A Next BFF route handler commonly forwards the
incoming request's headers wholesale — `headers: Object.fromEntries(request.headers)` — and under
this rule that call throws whenever the browser sent an `Authorization` header. That refusal is
correct and is close to the point of the rule: such a route should send the **session-bound** token
the SDK was given, not relay the client's. Naming "do not forward an incoming request's headers
wholesale" in the message turns a confusing failure into an instruction, for the consumer § 1.3
identifies as the first one to arrive.

### 3.6 Ordering against the empty-bearer refusal

The header check runs **first**, unconditionally, before `req.contextValues.get(authContextKey)`.

Two consequences, both intended. A request that is wrong in both ways — a caller header *and* an
empty bearer — reports the header, deterministically, rather than depending on evaluation order
nobody wrote down. And the check runs before `req.header.set` on the bearer branch, so it can never
trip over the header the SDK itself just wrote.

## 4. The change

One source file: `ts/packages/paigasus-sdk/src/transport.ts`. The check is added at the top of
`authInterceptor`, ahead of the existing body:

```ts
export const authInterceptor: Interceptor = (next) => async (req) => {
  if (req.header.has('authorization')) {
    throw new ConnectError('@paigasus/sdk: …', Code.InvalidArgument);
  }
  const auth = req.contextValues.get(authContextKey);
  // … unchanged, except that the empty-bearer refusal becomes a ConnectError too (§ 3.1) …
};
```

`ConnectError` and `Code` become value imports from `@connectrpc/connect`, which the file already
imports `createContextKey` from.

**A comment at the check records what § 3 decides**, because none of it is readable from the code:
that the rule applies to both arms on purpose; that it precedes the bearer `set` on purpose; that
the error is a `ConnectError` with `Code.InvalidArgument` so this package's own presentation table
renders it as `invalid-input` rather than falling through to `generic`; and that the message must
never carry the header's value.

**One invariant needs stating that § 3.6 does not cover.** § 3.6 reasons about ordering *inside*
`authInterceptor`. It says nothing about ordering *across* an interceptor array — and Connect
applies "the interceptor at the end of the array … first" (`dist/esm/interceptor.d.ts:28-29`), so a
second interceptor appended after `authInterceptor` would run **before** it and, if it set an
`authorization` header, would trip this refusal against the SDK's own writing. The guard already
exists: `tests/transport-wiring.test.ts:49` pins `expect(options.interceptors).toEqual([authInterceptor])`
to the exact one-element array. The comment names that test, so whoever adds a second interceptor
finds the constraint at the same time as the failure.

**The `Auth` doc comment carries the rule too** (`src/transport.ts:29-32`). A consumer reads the
source, not `docs/superpowers/specs/`, and the `Auth` union is where they meet the anonymous arm.
Two sentences: the client owns `authorization`, and a caller-supplied one is refused on both arms.

No public type changes. `Auth`, `TransportOptions`, `createIamClient` and `bindAuth` keep their
signatures. The transport cache, its key, and `disposeTransports` are untouched.

## 5. Tests

All in `ts/packages/paigasus-sdk/tests/transport.test.ts`, beside the existing empty-bearer
refusals, because that file is where interceptor behaviour is already proven and the enforcement
site is the interceptor.

**Two helper changes**, so the diff is predictable. `fakeUnaryRequest()` takes no arguments and
always returns `new Headers()` (`tests/transport.test.ts:100-106`); it gains an optional
`headers?: HeadersInit` parameter defaulting to none. A `fakeStreamRequest()` sibling is added for
case 10, identical but with `stream: true`. Case 6 additionally needs a **recording** `next` rather
than the shared `noopNext`, for the reason given below.

| # | Case | Expected |
| --- | --- | --- |
| 1 | `{ anonymous: true }` + `authorization` | rejects; message names `authorization` |
| 2 | `{ bearer: 'tok' }` + `authorization` | rejects — the § 1.1 bearer row |
| 3 | header spelled `Authorization` | rejects (M2) |
| 4 | header spelled `AUTHORIZATION` | rejects (M2) |
| 5a | `authorization: ''`, `{ anonymous: true }` | rejects (M4, § 3.4) |
| 5b | `authorization: ''`, `{ bearer: 'tok' }` | rejects — proves the check precedes `req.header.set` |
| 6 | `proxy-authorization` only, `{ anonymous: true }` | resolves; a recording `next` sees `proxy-authorization` intact and no `authorization` |
| 7 | `proxy-authorization` only, `{ bearer: 'tok' }` | resolves; `authorization` is `Bearer tok` **and** `proxy-authorization` survives |
| 8 | `authorization` + `{ bearer: '' }` | rejects naming the **header**, not the empty bearer (§ 3.6) |
| 9 | any refusal above | `err instanceof ConnectError` and `err.code === Code.InvalidArgument` (§ 3.1) |
| 10 | `fakeStreamRequest()` + `authorization` | rejects — the streaming path (M5) |
| 11 | `authorization: 'Bearer super-secret'` | message matches `/authorization/`, `/bearer/i`, `/anonymous/` and `/proxy-authorization/`, and `expect(message).not.toContain('super-secret')` (§ 3.5) |

Case 9 also covers the empty-bearer refusal, which § 3.1 moves to `ConnectError`.

**Case 6 needs a recording `next`, not `noopNext`.** The shared `noopNext` returns
`{ stream: false, header: req.header }` — the *same* `Headers` object (`tests/transport.test.ts:120`)
— so asserting on the returned header proves nothing about `next` having been called at all. The
case pushes `req.header.get('proxy-authorization')` into an array from inside `next` and asserts the
array holds exactly one entry.

The existing tests in that file — a bearer request with no caller header still gets
`Bearer <token>`, an anonymous one still gets none — stay as they are and serve as the regression
guard that the new check does not fire on a clean request.

### 5.1 Which cases actually detect a mutation

Cases 3 and 4 are near-vacuous and are kept as defence in depth, not as coverage. `req.header` is
typed `Headers` on both request interfaces, so `has` is case-insensitive by construction and no
plausible implementation fails them. They should not be counted when arguing that this rule is
well covered.

The cases that fail against a realistic wrong implementation are **1, 2, 5b, 8, 9, 10 and 11**:
1 and 2 against a rule applied to one arm only; 5b and 8 against a check placed after the
`contextValues.get` branch; 9 against a plain `Error`; 10 against a `!req.stream &&` guard; 11
against a message that echoes the credential. Cases 6 and 7 fail against an over-broad check
written on a substring or regex rather than an exact field name.

### 5.2 One seam stays untested, and this is why

The step from `CallOptions.headers` into `req.header` is Connect's, not this package's. M1 verifies
it by reading `request-header.js:21-22`; **no test in this repo exercises it.**

That seam is not unreachable. `requestHeader` is a real subpath export —
`@connectrpc/connect/package.json:41-44` maps `./protocol-grpc` and
`dist/esm/protocol-grpc/index.js:22` re-exports it — so a test could call
`requestHeader(true, undefined, { authorization: 'x' })` and assert `.has('authorization')`.

It is deliberately not written, for a reason that is worth more than the coverage:
`request-header.js:19` marks the export **`@private Internal code, does not follow semantic
versioning`**. A test pinned to it would couple this suite to a connect internal that may change on
a patch release, to prove a behaviour that belongs to connect rather than to this package. The
package's existing `tests/transport-wiring.test.ts` takes the same line — it asserts what
`getTransport` *passes* to `createGrpcTransport`, not what connect then does with it.

**The precise residual, narrowed.** What no test here reaches is the wiring hop from
`Transport.unary`'s `header` parameter into `requestHeaderWithCompression` — not the seeding
itself, which M1 reads directly. If a future `@connectrpc/connect` stopped seeding `req.header`
from `CallOptions.headers`, every test in § 5 would still pass while the rule stopped applying to
the path a caller actually uses. Re-read `request-header.js` and `transport.js:76,156` on a connect
major bump.

## 6. Documentation

Three edits beyond the spec file itself.

The code comment at the check, and the interceptor-order invariant, described in § 4.

The two-sentence rule on the `Auth` doc comment (`src/transport.ts:29-32`), also § 4 — the one
place a consumer reading the source meets the anonymous arm.

A forward reference in `docs/superpowers/specs/2026-09-09-sma-508-sdk-design.md` § 7.4, pointing at
this spec, so a reader of the older design does not conclude that its description of the anonymous
arm is still the whole contract.

**No CLAUDE.md entry.** The repo's CLAUDE.md carries repo-wide gotchas — build gates, toolchain
traps, cross-cutting invariants. A single package's public-API rule, enforced by that package's own
tests and stated on the type its callers already read, is not one.

## 7. Out of scope

**The chat client.** `createChatClient` builds its own `headers` object literal (`src/chat.ts:238`)
and accepts none from the caller — its per-call options are `{ signal }` alone (`src/chat.ts:169`).
It also takes `{ bearer: string }` with no anonymous arm (`src/chat.ts:197`). There is nothing for
this rule to reach.

**Widening the rule to other headers, or `Auth` to other schemes.** § 3.3.

**The `contextValues` refusal.** Unchanged, in `src/iam.ts`, and it stays a synchronous plain
`Error` for the reason § 3.1 gives.

**Exporting `getTransport`.** It stays package-private. That is what makes the enforcement-site
choice defence in depth rather than necessity (§ 3.2). If a later issue exports it — to let an app
build a client for a non-IAM Connect service over the same cached transport — the interceptor
becomes the only site that can enforce this rule, and that issue should cite this section rather
than rediscover it.

## 8. Acceptance criteria

1. `authInterceptor` throws when `req.header` already carries an `authorization` header, on both
   `Auth` arms, before it reads the bound `Auth` — on the unary and the streaming path alike.
2. The thrown value is a `ConnectError` with `code === Code.InvalidArgument`, and so is the existing
   empty-bearer refusal.
3. The message names `authorization`, both remedies (`{ bearer }` / `{ anonymous: true }`), and
   `proxy-authorization`; and it does **not** contain the caller's header value.
4. `proxy-authorization`, `cookie` and a custom header pass through unchanged, and a bearer request
   carrying `proxy-authorization` still receives its `Bearer` header.
5. A request carrying no `authorization` header behaves exactly as it does on `1865cf58`.
6. The eleven cases in § 5 are tests in `tests/transport.test.ts` and pass; the suite's total rises
   from 236 with no pre-existing test modified except where § 3.1 changes the empty-bearer error
   type.
7. The `Auth` doc comment and the check's comment carry what § 4 requires, and
   `tests/transport-wiring.test.ts:49` is named as the interceptor-order guard.
8. `moon ci` is green over the repository's full target list, `ts:fmt` included.

## 9. What the adversarial challenge changed

Every finding was verified against the source before being folded in. All were justified; none was
rejected.

| Finding | Change |
| --- | --- |
| BLOCKER — a plain `Error` becomes `Code.Unknown`, which this package's own table renders `generic`, blaming the service for a caller error | § 3.1 is new: throw `ConnectError(…, Code.InvalidArgument)`, and move the empty-bearer refusal with it. M6 added to show the code survives. Case 9 added; AC 2 added |
| BLOCKER — nothing forbade the message echoing the credential, and AC 2 had no test | § 3.5 is new and carries the prohibition; case 11 makes it testable; AC 3 carries it |
| MAJOR — § 3.1's rejection of the `bindAuth` site cited a bypass that `package.json`'s `exports` forbids, and a test line that says the opposite | § 3.2 rewritten on the two reasons that are true — `HeadersInit` normalisation and policy locality — and states plainly that `bindAuth` covers today's whole public surface |
| MAJOR — § 3.2 marked option 2's risk "answered" while covering only the intermediary half | § 3.3 states the real consequence: `Bearer` becomes the only scheme this SDK can send |
| MAJOR — the streaming path was never mentioned; all eight cases were unary | M5 added; case 10 added; AC 1 covers both arms |
| MAJOR — § 1.3's grep enumeration was missing six files | § 1.3 rewritten, leading with `private: true` / `0.0.0` as the fact that actually settles it, and naming `claims-resolver.ts` as the first planned consumer |
| MAJOR — M1 and M3 quoted the unshipped TypeScript source while citing CJS paths, and M1 skipped the `requestHeaderWithCompression` hop | § 2 now cites `dist/esm/` throughout with line numbers, and names the wrapper |
| MAJOR — § 5.1 called the seam unclosable when a subpath export reaches it | § 5.2 gives the real reason (the export is marked `@private`, no semver) and narrows the residual to the wiring hop |
| MINOR ×6 | Case 5 split into 5a/5b; case 6 given a recording `next`; § 5.1 added, marking cases 3–4 as non-detecting; AC 4 narrowed to a named header set; the interceptor-order invariant added to § 4; the helper changes stated in § 5 |
| QUESTION — a Next BFF forwarding headers wholesale now throws | § 3.5 folds that instruction into the error message |
| QUESTION — should the rule live on the source, not only in the spec? | § 4 and § 6: the `Auth` doc comment carries it |
| QUESTION — does the rule reach a future non-IAM client on `getTransport`? | § 7 records that `getTransport` stays package-private, and what changes if that is revisited |
| QUESTION — should the empty-bearer refusal move too? | Yes; § 3.1 decides it |
