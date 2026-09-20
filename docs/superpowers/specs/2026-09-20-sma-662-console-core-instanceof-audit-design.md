# SMA-662: audit @paigasus/console-core for cross-copy instanceof classification

- Issue: SMA-662, a follow-up that SMA-657 § 7 created.
- Related: SMA-657 (the same audit for `@paigasus/auth`, and the D7 rule this document applies),
  SMA-653 (`isSessionStoreUnavailable`, the first `code`-based predicate), SMA-511 Task 22 (the
  measured module duplication), SMA-648 and SMA-650 (the descriptor cache this audit reads),
  SMA-512 PR 2 (`createConsoleRuntime`).
- Package: `ts/packages/paigasus-console-core` (`@paigasus/console-core`).
- Revision 2: folds in the spec challenge. Every finding was verified against the code before it
  was accepted, and three were settled by MEASUREMENT rather than by argument. § 11 lists what
  changed. Revision 1's central test plan was wrong and is replaced.

## 1. Problem

Next gives a route handler and a page SEPARATE module graphs. That is MEASURED, on this
repository's own standalone build, and recorded in `ts/packages/paigasus-auth/src/runtime.ts:180`
as "MEASURED on Next 16.3.4". The tree is now pinned at `^16.3.5`
(`ts/pnpm-workspace.yaml:106`), so the foundational measurement predates the current pin by one
patch release. A class then has two identities in one process, and `instanceof` against the local
one is false for an object the other copy built.

SMA-657 fixed one such site in `@paigasus/auth` and audited that package. It did NOT audit
`@paigasus/console-core`. That package is not obviously safe: `src/discovery.ts:57-65` holds its
descriptor cache and its Redis client on `globalThis`, under
`Symbol.for('paigasus.console-core.discovery-state.v1')`, and its own comment says it copies
`@paigasus/auth`'s `getAuthRuntime` for the same reason. So console-core uses the exact mechanism
SMA-657 is about.

This document is that audit. **It finds no defect.** The headline result is stronger than "no
client crosses": the one site the issue singled out, `errors.ts:23`, is cross-copy safe **by
construction**, because `ConnectError` carries a `Symbol.hasInstance` brand check. That is measured
in § 3 row 1 and is the only genuinely new thing this issue ships a test for.

**The rule this audit applies (SMA-657 D7), quoted so this document is self-contained.** A class
thrown by a closure that is reachable through shared state, and caught OUTSIDE that closure, must
be classified by its `code`. A class thrown and caught inside one closure, or thrown and caught by
two modules of one copy, may use `instanceof`. The boundary is the CLOSURE, not whether state is
shared.

D7 is stated for a class the declaring package defines. Only ONE of the six sites below is such a
class — `discovery.ts:274`, on `DescriptorCacheTimeoutError` — so only that site falls squarely
under the rule. Three test a THIRD-PARTY class and two test the builtin `Error`; those five are
judged by analogy to it, and saying so plainly is clearer than stretching the rule.

## 2. Decisions

- **D1. Nothing changes behaviour.** No classification moves to a stable identity, because no site
  needs it. The issue's second acceptance criterion ("for each site that can cross…") has an empty
  subject.

- **D2. Each of the six sites gets a comment stating why it cannot cross**, using § 3's reason and
  preferring the strongest available argument. This is the third acceptance criterion. At the
  `errors.ts` site the brand argument replaces the closure argument, because the brand argument is
  strictly stronger — it holds even if a client did cross. The closure argument for that site
  remains available, in § 3 row 1's Path A and Path B.

- **D3. The sharing model is stated in a header block in `src/errors.ts`.** The full table stays in
  this document; the header states the model and points here. This mirrors how SMA-657 put its rule
  beside `hasAuthErrorCode` rather than in a README.

- **D4. ONE new test row, in `tests/unit/call-iam.test.ts`.** It asserts that `callIam` maps a
  STRUCTURALLY FOREIGN `ConnectError` — one whose prototype is not the local class's — instead of
  rethrowing it. § 6 gives it, and § 6 records the measurement that shows it passes.

  Revision 1 proposed a new `tests/unit/principal-resolver.test.ts` on the grounds that
  `createIntrospectPrincipalResolver` "has NO test today". **That was false.**
  `tests/integration/principal.test.ts` imports it at `:13` and drives it in EIGHT tests (`:38`,
  `:59`, `:78`, `:85`, `:100`, `:109`, `:117`, `:144`), in the Docker-free
  `paigasus-console-core-ts:test` tier — `vitest.config.ts:37` excludes only `tests/containers/**`.
  Two of revision 1's three proposed rows already existed verbatim: its happy path is
  `principal.test.ts:35-50` (which additionally pins the call ORDER), and its same-copy
  `ConnectError` row is `:93-103`. The resolver is well covered, and no row is added there.

- **D5. `callIam` is NOT changed.** D7 prescribes classifying by `code`, and that prescription is
  unavailable here: `ConnectError.code` is already taken — it is the gRPC status, not an identity.
  No change is needed anyway, because the brand check in § 3 row 1 already does the job that a
  `code` predicate would do, and it does it inside the dependency rather than in this package.

- **D6. Per-copy `createConsoleRuntime` is intended, not a second defect.** § 5 answers the issue's
  Note.

- **D7. No gate.** Nothing reds when a future console-core site classifies a crossing class with
  `instanceof`. This is the same residual SMA-657 D5 accepted, for the same reason: a gate would
  have to tell a crossing site from a non-crossing one, which needs § 3's call-graph reasoning and
  not a syntactic rule. § 9 records it, with three more residuals the challenge surfaced.

## 3. The audit: every `instanceof` in `src/`

`src/` holds SIX `instanceof` operators. One tests a class this package defines, three test a
third-party class, and two test the builtin `Error`. **No site's classification can fail across
the copies.** Note the careful wording: one of the errors audited here — `DescriptorCacheTimeoutError`
— DOES cross into the other copy (row 3). What stays inside one copy is the `instanceof` that
classifies it, which is the property this audit is about.

Line numbers in the table below are pre-change; post-merge they are `errors.ts:46`,
`discovery.ts:109`, `:110`, and `:286`.

| Site | Right side | Category | Crosses? | Why |
| --- | --- | --- | --- | --- |
| `errors.ts:23` | `ConnectError` | third party | No | The class carries a `Symbol.hasInstance` brand check. Row 1. |
| `discovery.ts:102` | `SocketTimeoutError` | third party | No | The listener is registered by the call that created the client. Row 2. |
| `discovery.ts:103` | `SocketClosedUnexpectedlyError` | third party | No | Same as row 2. |
| `discovery.ts:274` | `DescriptorCacheTimeoutError` | package | No | Thrown and caught inside one closure. Row 3. |
| `principal-resolver.ts:83` | `Error` | builtin | No | One isolate, one builtin. |
| `principal-resolver.ts:84` | `Error` | builtin | No | One isolate, one builtin. |

Seven more operators live OUTSIDE `src/`: `testing/fake-iam.ts:177` (`errorInfoOf`) and `:190`
(`stampCorrelation`), both on `ConnectError`; `tests/unit/testing-tls.test.ts:30`,
`tests/unit/authorize.test.ts:17`, `tests/containers/descriptor-cache-idle.test.ts:101` and
`:210`; and `tests/unit/call-iam.test.ts`'s own new row (this issue's — see § 6), also on
`ConnectError`. `testing/` is a published subpath that both apps' test harnesses import, never app
code, and a harness runs in one process with one module graph. All seven are out of scope (§ 9).

### Row 1 — `errors.ts:23`, `err instanceof ConnectError` inside `callIam`

**`ConnectError` is cross-copy safe by construction. MEASURED.** The class defines a static
`Symbol.hasInstance`
(`@connectrpc/connect@2.2.0`, `dist/esm/connect-error.js:82-98`):

```js
static [Symbol.hasInstance](v) {
    if (!(v instanceof Error)) return false;
    if (Object.getPrototypeOf(v) === ConnectError.prototype) return true;
    return (v.name === "ConnectError" && "code" in v && typeof v.code === "number" &&
        "metadata" in v && "details" in v && Array.isArray(v.details) &&
        "rawMessage" in v && typeof v.rawMessage == "string" && "cause" in v);
}
```

So `instanceof ConnectError` is NOT a prototype test. It falls back to a duck-type BRAND: the
`name`, plus five fields. An instance from a second copy of the package satisfies every clause, so
`errors.ts:23` is true for it. connect-es carries this guard for exactly the multiple-copies
problem this issue is about.

Two measurements taken on this tree, both recorded in § 6:

- An object whose prototype is NOT `ConnectError.prototype`, built to the brand's shape, is
  `instanceof ConnectError` — `true`.
- `callIam` given such an object returns `{ ok: false }` with `presentation: 'forbidden'` and
  `transport: { kind: 'grpc', code: 7, codeName: 'PermissionDenied' }`. It maps it; it does not
  rethrow it.

Note what this widening means, in the spirit of SMA-657 D8: any `Error` carrying the brand's shape
is admitted, not only a real `ConnectError`. That widening IS the mechanism, and it is the
dependency's decision, not this package's.

**The call-graph argument, which is now the secondary reason.** Even without the brand, no client
built by one copy has its errors classified by another. Two paths:

*Path A — a page, a layout, a Server Action or a `load.ts`.* Every such caller gets its clients
from its own module graph's `lib/console.ts` and imports `callIam` from the same graph. The chain
has THREE hops, not two: `iam-clients.ts` never constructs a `ConnectError` and does not import the
class (`:12` imports only `type CallOptions, Client`). The error is built by `@paigasus/sdk` —
`src/transport.ts:105` and `:123` throw one directly, and `createGrpcTransport` makes
`@connectrpc/connect-node` produce the rest — against **the SDK's own** `@connectrpc/connect`
import. So the chain is console-core's connect ≡ the SDK's connect ≡ connect-node's peer connect.
It holds today because `ts/pnpm-lock.yaml` has exactly ONE peer variant,
`@connectrpc/connect@2.2.0(@bufbuild/protobuf@2.15.0)`, and no app sets `serverExternalPackages`
(`paigasus-next-config/src/index.ts:83-96` sets only `transpilePackages`). § 7 records that as an
assumption and § 9 as a residual. This is D7's second allowed case.

*Path B — the principal resolver, which DOES live on shared state.* The app's `lib/auth.ts:30`
passes `createIntrospectPrincipalResolver({ clientsForToken: async (token) =>
iamClientsForToken(token, await requestCorrelationId()), logger })` to `getAuthRuntime`, a process
singleton whose FIRST call fixes the resolver for the life of the process. So copy A's resolver
serves requests that copy B handles: `@paigasus/auth`'s `http/routes.ts:293` calls
`runtime.resolver.resolve(...)` in the login callback.

That is exactly a closure reachable through shared state, and it is still safe, because the closure
is SELF-CONTAINED: it supplies both sides. `deps.clientsForToken` is copy A's `iamClientsForToken`,
so the client is copy A's; `callIam` at `principal-resolver.ts:62` and `:65` is copy A's import, so
the classifier is copy A's; and the broad catch at `:79` is inside the same `resolve` closure. Copy
B contributes nothing but the call. This is D7's first allowed case.

**Citation fix.** `principal-resolver.ts:4` says the login callback calls the resolver at
`routes.ts:236`. That is stale; the call is at `:293`. § 8 corrects the comment.

### Row 2 — `discovery.ts:102` and `:103`, the node-redis classes

`connectionLossReason` runs only inside the `error` listener that `watchConnectionLoss` registers
(`discovery.ts:126-130`). `watchConnectionLoss` is called from ONE production site, `:312`, in the
same call to `redisDescriptorCache` that created the client at `:301`. So the listener is always
registered on the client that the same call created: the copy whose `redis` import produced the
error object is the copy whose `redis` import the `instanceof` resolves.

**That pairing is the whole argument, and it holds however many clients exist.** Revision 1 also
claimed `redisDescriptorCache` "runs at most once per process" and that "there is no race". The
first half is falsifiable: `resetDiscoveryForTest` is exported from the package ROOT entry
(`src/index.ts:22`) and is called by five test files, and after it runs `state().processCache` is
`undefined` (`:356`), so the next `descriptorCacheFor` from EITHER copy builds a second client and
a second listener. The rows stay safe, because each listener is still registered by the call that
created its client. The weaker, unbreakable argument is the one that goes into the comment.

The single-owner property is still true in production and is worth stating: `descriptorCacheFor`
returns early at `:332`, and the path from `:331` to `:337` contains no `await`, so two copies
cannot interleave between the read and the write.

### Row 3 — `discovery.ts:274`, `DescriptorCacheTimeoutError`

Thrown at `:258` (`circuit-open`) and `:269` (`deadline`), caught at `:274`, all inside the one
`bounded` closure that begins at `:248`. That closure is built once, by whichever copy first called
`descriptorCacheFor`, and the other copy reaches it through `globalThis`. D7's first allowed case.

The error DOES escape that closure — `:284` rethrows it — and crosses into the other copy. That is
harmless, because nothing outside classifies it. `@paigasus/discovery` bare-catches on every path a
cache error can take and names no class: `core/single-flight.ts:256`, `:277`, `:293`, `:333`,
`:340`, plus `probeAndStore` and `finishRevalidation`. The STALE path returns `stale` (`:323`), not
`cache-unavailable`.

**The `after()` path is closed, and this is the first thing a reviewer asks.** Background
revalidation is handed to `deps.waitUntil` — `runtime.ts:108` passes `(p) => after(p)` — but
`single-flight.ts:304` attaches a `.catch(...)` BEFORE the promise reaches `waitUntil`, so a
`DescriptorCacheTimeoutError` there is handled and never becomes an unhandled rejection.

`DescriptorCacheTimeoutError` is exported from `src/discovery.ts` but NOT from `src/index.ts`, so
no consumer can classify it. The only `instanceof` on it outside `src/` is
`tests/containers/descriptor-cache-idle.test.ts:210`, which statically imports one copy.

### Classification WITHOUT `instanceof`

The issue is about identity crossing two module copies. `instanceof` is one symptom; an audit that
enumerates only the token can be complete and still miss the next defect. `src/` holds four other
classification sites, and all four are safe:

| Site | Shape | Verdict | Why |
| --- | --- | --- | --- |
| `correlation.ts:42-45` | `unstable_rethrow(err)` | Safe | Next's own check is digest-based and `$$typeof`-based, not a class test, so a control-flow error built in either graph is recognised. |
| `principal.ts:42` | `answer.error.reason === ErrorReason.IDENTITY_NOT_PROVISIONED` | Safe | A string enum value compared by value. Values do not have identity. This is the `code`-shaped classification D7 prescribes. |
| `discovery.ts:127` | `client.isOpen` / `client.isReady` | Safe | A duck read of two booleans on a client held on `globalThis`. Property reads carry no class identity. |
| `iam-clients.ts:38` | `typeof value !== 'function'` | Safe | A `typeof` test through a Proxy over the SDK's Proxy. No identity involved. |

## 4. `createConsoleRuntime`'s sharing model

This is the issue's fourth acceptance criterion.

**Every product is built fresh per call.** `runtime.ts` reads no `globalThis` and holds no
module-level state. `createConsoleRuntime` (`runtime.ts:47`) returns ten closures over its own
`deps`; eight are React `cache()` wrappers and two (`iamClientsForToken`, `iamClientsForAction`)
are not.

**Three things are per PROCESS or per COPY rather than per call, and all three are reached THROUGH
a per-copy product:**

1. **The descriptor cache and the Redis client — one per process.** `discovery()`
   (`runtime.ts:108`) builds a fresh `Discovery` handle per request, but `createAppDiscovery`
   passes it `descriptorCacheFor(...)`, the `globalThis` singleton. The handle is per copy and per
   request; the cache beneath it is one per process. That is the wanted behaviour, stated at
   `discovery.ts:50-56`: one cache for every zone composed into the process.
2. **The `AuthRuntime` — one per zone per process.** `deps.authRuntime()` reaches
   `@paigasus/auth`'s singleton, keyed by zone: `runtime.ts:203` builds the globalThis symbol from
   `` Symbol.for(`${RUNTIME_KEY_PREFIX}:${zone}`) ``, so a process composing two zones holds two.
   Its first call per zone fixes FOUR object-valued fields, not two: `store`, `resolver`, `logger`
   and `oidc`, plus every scalar (`paigasus-auth/src/runtime.ts:155-172`). `store` is the field
   whose cross-copy identity produced SMA-653, and `oidc` the one that produced SMA-657; both are
   fixed inside `@paigasus/auth` and neither reaches a console-core classifier. `resolver` is
   console-core's own, and row 1 Path B covers it.
3. **The SDK's transport cache — one per COPY, not per process.** `@paigasus/sdk`'s `getTransport`
   reads a module-level `Map` (`sdk/src/transport.ts:215`) and holds an `Http2SessionManager` per
   entry. Under two graphs each copy opens its own HTTP/2 session pool to IAM, and
   `disposeTransports()` reaches only the copy that calls it. `iam-clients.ts:8-9` calls the
   transport "process-scoped", which is wrong under the two-copy model; § 8 corrects it. This costs
   one extra connection pool. It is not a correctness defect and this issue does not change it.

**The IAM clients themselves never cross.** `iamClientsForToken` (`runtime.ts:54`) closes over
`deps.config()` only and returns a value; nothing stores a client on shared state.

**So: can a client built by one copy have its errors classified by another?** Row 1 answers it
twice — no by the call graph, and harmless even if it did, because of the brand.

## 5. Is per-copy `createConsoleRuntime` a second defect?

No. It is intended.

`runtime.ts:11` says "CALL THIS EXACTLY ONCE PER APP, AT MODULE SCOPE", because `cache()` memoizes
by the WRAPPER's identity, so a second call makes a second memoization identity. The apps'
`lib/console.ts:3` states it more precisely: "Called once per module graph".

Those readings agree, because the invariant is PER REQUEST. A page render and a route-handler
request are different requests, served by different graphs, and each request is served by one
copy. Two copies, each with one `createConsoleRuntime` call, therefore give each request exactly
one memoization identity.

**What R11 does and does not control.** `gateway-console/tests/e2e/two-zone-runtime.spec.ts:45-55`
groups Introspect calls by the per-request correlation id and fails any group holding more than
one. That controls the PER-REQUEST invariant — the property D6 preserves. It cannot observe "two
copies, one call each", because those serve different requests with different ids. Revision 1
called R11 a gate on D6; it is a gate on the invariant, which is the thing worth gating.

**A caveat for a future reader of an R11 red.** `introspectWithProvisioning` (`src/principal.ts:41-48`)
makes TWO `authn.introspect` calls under ONE correlation id when a token is unprovisioned, which is
what R11's final assertion refuses. It does not fire today only because the login callback
provisions with `getServiceInfo` first. An R11 red is therefore not automatically a second
`createConsoleRuntime` call.

## 6. Tests

One new row, in `ts/packages/paigasus-console-core/tests/unit/call-iam.test.ts`. Vitest, no Docker,
under `paigasus-console-core-ts:test`. That task MERGES with `.moon/tasks/typescript-project.yml`,
so `@group(sources)` (`src/**/*`) and `@group(tests)` (`tests/**/*`) already cover it. **No
`moon.yml` change and no `vitest.config.ts` change is needed** — and if a future row ever needed
one, `vitest.config.ts` is already an input of the `test` task (`moon.yml:46`).

**The row.** A helper builds an object to `ConnectError`'s brand shape — `name: 'ConnectError'`,
`code`, `metadata`, `details: []`, `rawMessage`, `cause`, and a `findDetails()` returning `[]`,
which `mapError` calls — from a class declared locally in the test, so its prototype is NOT
`ConnectError.prototype`. That is what a second copy's instance looks like to the local class.

Assertions, in order:

1. The precondition, FIRST, or the row passes vacuously:
   `expect(Object.getPrototypeOf(err)).not.toBe(ConnectError.prototype)`.
2. The brand admits it: `expect(err instanceof ConnectError).toBe(true)`.
3. `callIam` MAPS it rather than rethrowing: the result is `ok: false` with
   `presentation: 'forbidden'` and `transport: { kind: 'grpc', code: 7, codeName: 'PermissionDenied' }`.

**MEASURED, on this tree, before this document was accepted.** A throwaway probe under
`paigasus-console-core-ts`'s own vitest config produced exactly those three results
(`protoIsLocal? false`, `instanceof? true`, `ok? false presentation= forbidden transportCode=
{"kind":"grpc","code":7,"codeName":"PermissionDenied"}`). The probe was deleted. The row is a
transcription of a measurement, not a prediction.

**What the row is for.** It pins a THIRD-PARTY behaviour this package's safety now rests on. If a
connect-es major release drops `Symbol.hasInstance`, or narrows its brand, this row reds and the
audit's headline conclusion is re-opened deliberately. Nothing else in the repository would notice.

**Mutation check.** Reverted one at a time; the row must red:

- `errors.ts:23`'s guard changed to a prototype test
  (`Object.getPrototypeOf(err) !== ConnectError.prototype`) — the row reds, because `callIam` then
  rethrows. This is the mutation that proves the row tests the brand and not merely `callIam`.
- the brand-shaped fixture stripped of one required field (`rawMessage`, say) — the row reds at
  assertion 2, which proves the fixture is load-bearing and not accidentally a real `ConnectError`.

Both mutations compile. Revision 1's four-item list is withdrawn: one entry
("the `degraded(...)` return at `:63` replaced by a throw") produced no red, because no proposed row
made `getServiceInfo` fail, and three others were absorbed by `principal.test.ts`'s existing exact
`toEqual` assertions, so "at least one row reds" was satisfied without the new rows biting at all.

**Red first does not apply.** The row describes behaviour that already works, and it exists to pin
a dependency's contract. The mutation list above is the evidence that it bites, and the pull
request records the observed reds.

**What this row does NOT prove.** It proves the brand admits a structurally foreign instance in one
V8 isolate. It does not build a second real module copy of `@connectrpc/connect` — see § 10 for the
measurement that rules that approach out. It does not prove Next puts the two layers in two copies;
`@paigasus/auth`'s `src/runtime.ts` measured that separately.

**No test is added for rows 2 and 3.** Both are argued from code visible in one file each, and
`@paigasus/discovery` already owns tests for its own cache-failure paths. **No row is added for the
principal resolver** (D4): it already has eight.

## 7. Assumptions

- The two module copies run in ONE V8 isolate and share the builtin `Error`. This is what makes
  `principal-resolver.ts:83-84` safe, what the brand's own first clause (`v instanceof Error`)
  needs, and what `@paigasus/auth`'s shipped `isSessionStoreUnavailable` already relies on.
- **Exactly one `@connectrpc/connect` peer variant is installed, and no app sets
  `serverExternalPackages`.** Row 1 Path A rests on this. Both hold today
  (`ts/pnpm-lock.yaml`; `paigasus-next-config/src/index.ts:83-96`). Path A is the secondary
  argument, so a break here does not by itself re-open row 1 — the brand still holds.
- A bundler resolves one import specifier, from the modules of one graph, to one instance. A
  property of every bundler this repository uses, not a measurement taken here.
- **Copy A's request-scoped code works inside a request copy B serves.** Path B runs copy A's
  `requestCorrelationId()` — and therefore copy A's `next/headers` — during a request the
  route-handler graph is serving. Next externalizes `work-unit-async-storage.external.js` so that
  the request store IS shared across graphs, which is why this works. It is NOT measured here, and
  nothing in the repository measures it: `principal.test.ts:56-70` proves the id flows in a unit
  fixture, which cannot see the two-graph case. If the premise were false, `headers()` would throw,
  `unstable_rethrow` would pass it through, `requestCorrelationId()` would answer `null`, and
  `lib/auth.ts:12-18`'s promise that the login calls carry the request's id would be quietly false.
  That is a logging defect, not a correctness one, and it is out of scope. § 9 records it.

## 8. Documentation changes

Line numbers are pre-change.

- **`src/errors.ts`** gains a header block stating § 4's sharing model in short, and row 1's
  finding: `ConnectError` carries a `Symbol.hasInstance` brand, quoted in one line, so this site is
  safe across copies by construction. It names SMA-657 D7 and points at this document.
- **`src/errors.ts:23`** gains a one-line reason naming the brand.
- **`src/discovery.ts`**, above `connectionLossReason` (the doc block at `:95-100`), gains row 2's
  reason in its unbreakable form: the listener is registered by the call that created its client,
  so their `redis` imports always agree, however many clients exist.
- **`src/discovery.ts:274`** gains row 3's reason: throw and catch are inside the one `bounded`
  closure, and the error that escapes is classified by nobody.
- **`src/principal-resolver.ts`** gains, in its header, the statement that this resolver is the one
  product of this package that lives on shared state, and that its closure supplies both the client
  and the classifier. Its stale `routes.ts:236` citation is corrected to `:293`.
- **`src/iam-clients.ts:8-9`** is corrected: the SDK transport cache is per module COPY, not
  process-scoped (§ 4 item 3).
- **No CLAUDE.md change.** The rule lives beside the code, and CLAUDE.md already records the module
  duplication through the SMA-511 entries.

## 9. Out of scope, and the residuals

- **No gate (D7).** Nothing reds when a future console-core site classifies a crossing class with
  `instanceof`. SMA-657 D5 accepted the same residual, so the gap is now two packages wide.
- **Nothing gates the resolution premise.** Row 1 Path A depends on one `@connectrpc/connect` peer
  variant and row 2 on one `redis`. A Dependabot bump that forks either into two variants would
  weaken an argument in this document and red nothing. Row 1 survives it because of the brand; row
  2 does not have that backstop. No control is added — a lockfile assertion for two packages is out
  of proportion to the risk — but the exposure is now written down.
- **The two-graph `next/headers` premise is unmeasured** (§ 7). Its failure mode is a lost
  correlation id on the login callback's two IAM calls, not a wrong classification.
- **The seven `instanceof` operators outside `src/` are not audited.** Test harness and tests, one
  process, one module graph. If `testing/` ever gains a module app code imports, the two in
  `fake-iam.ts` come back into scope.
- **This audit is a snapshot.** A future edit that stores a client, a `Discovery` handle or any
  other closure on `globalThis` re-opens every row, and nothing will say so.
- **The blast radius of a hypothetical rethrow differs by call site, and § 3 does not enumerate
  it.** If `callIam` ever did rethrow a foreign error: a `load.ts` gives an HTTP 500; a Server
  Action through `iamClientsForAction` escapes `toActionResult`; and `createMayI`'s `ask`
  (`src/authorize.ts:78`) throws out of a `cache()`d accessor and poisons `mayI` for the rest of
  the request. The brand makes this unreachable, so it is recorded rather than analysed.
- **The SDK's per-copy transport cache is not changed** (§ 4 item 3). One extra HTTP/2 pool.

## 10. Rejected alternatives

- **A second real module copy of `@connectrpc/connect`, the pattern `@paigasus/auth` uses.**
  **MEASURED and rejected.** `vi.resetModules()` plus `await import('@connectrpc/connect')` returns
  the SAME class under this package's vitest config: `foreign.ConnectError !== ConnectError` is
  `false` and `instanceof` is `true`. Vitest externalizes node_modules, and `vi.resetModules()`
  clears Vite's registry, not Node's native ESM cache. Every existing two-copy test in this
  repository duplicates an INTERNAL source module, never a dependency. Revision 1 specified this
  row anyway; it would have failed its own precondition and shipped permanently red.
- **Adding `server.deps.inline: ['@connectrpc/connect']` to get real duplication.** **MEASURED: it
  works** — with it, `foreign.ConnectError !== ConnectError` becomes `true`. Rejected anyway,
  because `instanceof` stays `true` through the brand, so the inlined row proves exactly what § 6's
  row proves while changing module resolution for every test in the package.
- **Hardening `callIam` to a `code`-based predicate** (D5). `code` is taken, and the brand already
  does the work.
- **Giving `DescriptorCacheTimeoutError` a `code` and an exported predicate.** It cannot cross, and
  no consumer can name the class, so a predicate would have no caller.
- **A lint gate banning `instanceof` on a non-builtin.** It would report all six correct lines,
  need six disable comments, and teach the next author to add a seventh instead of thinking.
- **A new `tests/unit/principal-resolver.test.ts`** (D4). Eight tests already exist.

## 11. What the spec challenge changed

Every finding was verified against the code before it was accepted. Three were settled by
measurement.

- **Blocker.** D4 claimed `createIntrospectPrincipalResolver` has no test. FALSE — it has eight
  (`tests/integration/principal.test.ts`), and two of the three proposed rows already existed
  verbatim. The new test file is withdrawn.
- **Blocker.** § 6's one non-duplicate row rested on `vi.resetModules()` duplicating an EXTERNAL
  package. Measured: it does not. The row is replaced by a brand-shaped fixture, which needs no
  module duplication.
- **Blocker.** The mutation list was unverified: one entry produced no red, and three were absorbed
  by existing tests, so the check could not tell a working row from a vacuous one. Replaced with
  two mutations scoped to the new row.
- Row 1's whole argument was upgraded. Chasing the challenge's question about which
  `@connectrpc/connect` builds the error led to `Symbol.hasInstance`, which makes the site safe by
  construction rather than by a call-graph snapshot. Path A's three-hop chain through
  `@paigasus/sdk` replaces the two-hop claim, which named a module that does not import the class.
- § 4 named two fixed `AuthRuntime` fields; there are four, `store` and `oidc` included. The SDK's
  per-copy transport cache is added as a third shared item.
- Row 2's "at most once per process / no race" is falsifiable through the exported
  `resetDiscoveryForTest`. The comment now carries the unbreakable pairing argument instead.
- § 5 called R11 a gate on D6; it gates the per-request invariant. The provisioning-retry caveat is
  added.
- A new § 3 block covers classification WITHOUT `instanceof` — four sites, all safe.
- Corrections: Next is `^16.3.5`, not 16.3.4; six `instanceof` operators outside `src/`, not two (this branch's own new row later made it seven, see § 3);
  `createIamClients` is at `iam-clients.ts:49`; the discovery doc block is `:95-100`; the STALE
  path returns `stale`; `single-flight.ts:304`'s `.catch` closes the `after()` question; the
  `SessionStoreUnavailable` paragraph named a `callIam` path that does not exist and is dropped;
  D5 now says why `code` is unavailable.
