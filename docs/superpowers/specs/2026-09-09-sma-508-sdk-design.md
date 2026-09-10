<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-508 — `@paigasus/sdk`: Connect-ES gRPC clients and error mapping

**Issue:** [SMA-508](https://linear.app/smaschek/issue/SMA-508)
**ADR:** ADR-0018 (Connect-ES over gRPC; no OpenAPI surface), ADR-0019 (Canonical error model, incl. Amendment A1)
**Design source:** Frontend Architecture Scoping §§ 6, 7
**Date:** 2026-09-09
**Revision:** 2 — after the adversarial spec challenge. § 15 records what changed and what was rejected.

**Versions this spec is written against.** `@connectrpc/connect` 2.2.0, `@connectrpc/connect-node`
2.2.0, `@bufbuild/protobuf` 2.14.1, protoc-gen-es v2.13.0 (pinned in `buf.gen.yaml`), buf 1.70.0,
TypeScript 6.0.3, vitest 5.0.0, Node 24.16.0, Moon 2.5.3, pnpm 11.3.0.

---

## 1. Problem

`ts/packages/paigasus-sdk/src/index.ts` is `export {};`. The console has no way to call IAM or the
gateway.

SMA-504 gave every IAM gRPC error a `google.rpc.ErrorInfo` detail and every HTTP error a canonical
kebab `code`. SMA-498 declared the vocabulary — 57 reasons and 2 domains — in
`contracts/proto/paigasus/common/v1/error.proto`. Nothing in TypeScript reads any of it. ADR-0019
decision 9 requires consumers to branch on `(domain, reason)`; there is no consumer to do so.

## 2. The principle this package encodes

**The contract is the source of truth, in both directions.** Request and response types come from
the generated descriptors. Error identity comes from the generated registry. Nothing in this package
hand-writes a shape that `contracts/` already describes, and nothing branches on a human-readable
string.

The one deliberate exception is the gateway's OpenAI-compatible chat endpoint, which is a *product*
compatibility surface and will never be proto (ADR-0018 consequence 7). It gets a hand-written
client, and that is the whole hand-written surface.

## 3. Scope

A real `@paigasus/sdk` (`ts/packages/paigasus-sdk`, already scaffolded), plus a widened public
surface on `@paigasus/proto`, plus a second TypeScript-only `buf generate` invocation producing the
`google.rpc.ErrorInfo` descriptor, plus eight registration edits (§ 11).

Every new source file carries an SPDX header. `private: true` and source-only, matching
`@paigasus/ui` and `@paigasus/next-config`.

**§ 14.1 recommends splitting this into three PRs.** The section boundaries below are drawn so that
split is clean.

### 3.1 Layout

```
ts/packages/paigasus-proto/          (existing package, widened)
  src/error.ts                       asWireReason / fromWireReason — the codec (§ 9.2)
  src/error.test.ts                  parity with the Rust rejection set
  src/generated/google/rpc/error_details_pb.ts   NEW, generated (§ 5)

ts/packages/paigasus-sdk/
  package.json          exports ".", "./iam", "./chat", "./errors", "./errors/types"
  moon.yml              dependsOn + the inputs that actually confer affectedness (§ 11.1)
  tsconfig.json         types: ["node"]  (§ 6.4)
  vitest.config.ts      node env; ssr.resolve.conditions for server-only
  src/
    index.ts            server-guarded; re-exports "./iam", "./chat", "./errors"
    server-guard.ts     the single `import 'server-only'` site (§ 6.2)
    transport.ts        per-key transport cache (§ 7)
    iam.ts              typed client factories over the seven IAM services
    chat.ts             the OpenAI-compatible chat client + createTerminalFrameParser (§ 8)
    errors/
      types.ts          PaigasusError, Presentation — NO server guard (§ 6.3)
      presentation.ts   the reason -> presentation override table (§ 9.3)
      transport-status.ts  gRPC Code and HTTP status -> Presentation (§ 9.2)
      map-error.ts      mapError() — server-guarded
  tests/
```

Tests live in `tests/`, matching `@paigasus/ui` and `@paigasus/next-config`. The two files added to
`@paigasus/proto` follow *that* package's colocated `*.test.ts` convention instead — a package keeps
its own habit.

## 4. Three corrections this spec makes to its own inputs

Stated up front because two of them change the acceptance criteria.

### 4.1 ADR-0018 decision 5's premise is stale — the HTTP slice is one endpoint

Decision 5 gives the hand-written fetch client four surfaces, on the stated grounds that they have
"no proto service". Three of the four now do, and all three are implemented and mounted on IAM's
gRPC router (`rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs:80-109`):

| ADR-0018 decision 5 surface | Proto service today | Mounted at |
|---|---|---|
| `/v1/users` | `UserService.CreateUser` (`iam.proto:521-536`) | `mod.rs:94` |
| `/v1/outbox/dead-letters/*` | `OutboxService`, four RPCs (`iam.proto:613-687`) | `mod.rs:101` |
| system-policy retire | `AuthorizationService.RetireSystemPolicy` (`iam.proto:356-367`) | `mod.rs:87` |
| gateway chat | none, and never will have one | — |

So those three are reached over gRPC like everything else, and `@paigasus/sdk/chat` is the entire
hand-written surface. This *shrinks* the drift risk ADR-0018 names as its own main downside.

**This needs an ADR-0018 amendment.** Decision 5 itself — "HTTP-only surfaces get a small
hand-written fetch client" — stands unchanged; only its membership shrinks. Whether that Notion edit
precedes or follows this PR is an open decision (§ 16 Q1); a Linear comment is not an ADR edit.

### 4.2 AC 5 quotes a stale expected set, and "every downstream app" is empty

AC 5 quotes the `contracts->proto` expected set as six ids. It has **seven** — it gained
`paigasus-service-info-rs` after the issue was written (`ci/affected-graph/run.sh:258-259`).

**MEASURED (M4).** With the `sdk -> proto` edge in place, the set is exactly those seven plus
`paigasus-sdk-ts`. No app enters, because no project in `ts/` declares a `dependsOn` on
`paigasus-sdk-ts` or lists `@paigasus/sdk` in a `package.json`. AC 5's "and every downstream app" is
vacuous today, and this spec does not wire the console to the SDK — that is SMA-509/SMA-510's work.

Also measured: TypeScript project edges are **not** auto-derived. `.moon/toolchains.yml` and
`.moon/workspace.yml` carry no node dependency-sync setting, and every TS `dependsOn` in the repo is
hand-authored. This is the opposite of the Rust toolchain's behaviour for `path =` deps.

### 4.3 The registry is 57 reasons, not 46

SMA-507's handoff comment says "the registry currently holds 46 reasons". It holds **57** plus the
`UNSPECIFIED` sentinel (`error.proto:59-262`; the Rust mirror test asserts 57 at
`rs/crates/libs/paigasus-proto/src/error.rs:230`).

## 5. `google.rpc.ErrorInfo` in TypeScript — ADR-0019 A1.4 resolved

A1.4 left this open: *"the TypeScript path is unverified — if Connect-ES requires a generated
`ErrorInfo` descriptor, that module has to be generated after all."*

**It does (M3).** `ConnectError.findDetails` is
`findDetails<Desc extends DescMessage>(desc: Desc): MessageShape<Desc>[]`
(`connect-error.d.ts:84-85`). The second overload takes a `Registry`, also built from descriptors.
There is no schema-free path.

**MEASURED (M1).** `contracts/buf.yaml:6-9` already declares `buf.build/googleapis/googleapis` with
a `buf.lock` entry, and

```
buf generate --template buf.gen.googleapis.yaml \
  buf.build/googleapis/googleapis --path google/rpc/error_details.proto
```

exits 0 and emits exactly **one** file, `google/rpc/error_details_pb.ts` (662 lines). Its only
non-generated import is `Duration` from `@bufbuild/protobuf/wkt`, which is runtime, not a second
generated module. It exports `ErrorInfoSchema` alongside thirteen sibling `google.rpc` schemas.

**The command above is the measurement as taken, and it is NOT the form to ship.** It names the
module without a commit, which resolves to BSR HEAD. What `contracts/moon.yml` runs pins the
commit — `buf.build/googleapis/googleapis:c17df5b2beca46928cc87d5656bd5343` — for the reason
§ 5.2 gives. Do not copy this block into a task.

A1.4's fear was that referencing `ErrorInfo` from `error.proto` would emit Rust and Python pointing
at modules the run never produced. That fear was correct and this approach does not trigger it:
`error.proto` still imports nothing, and the second template runs **only** the TypeScript plugin
against a different input module.

**`--path`, not `--type`.** `--type google.rpc.ErrorInfo` would emit a smaller file. `--path` is
chosen because the fourteen schemas are one tree-shakeable module, and because a future consumer of
`RetryInfo` or `BadRequest` — both plausible for this error model — then needs no codegen change.
The cost is 662 generated lines nobody reads.

### 5.1 The ordering constraint, and what actually controls it

`contracts/buf.gen.yaml:3` sets `clean: true`, which wipes each `out:` directory before
regeneration. The googleapis template writes into the *same* tree
(`ts/packages/paigasus-proto/src/generated/`). So the main template must run **first**, and the
googleapis template **second** with `clean: false`. Reversed, the second run's output is destroyed.

`contracts:generate` therefore becomes a `script:` block with an explicit `set -euo pipefail` — Moon
does not enable errexit for `script:` blocks and takes the block's status from its last command, so
without it a failed first `buf generate` followed by a successful second exits 0.

**Revision 1 named the wrong control and is corrected here.** It claimed a `@paigasus/proto` test
would catch an ordering regression "before the codegen-drift gate would even run". That is false in
CI. The ordering lives in `contracts/moon.yml`, which is **not** among `contracts:generate`'s own
inputs, so a `moon.yml`-only edit selects neither `contracts:generate` nor `paigasus-proto-ts:test`.

The real control is the **codegen-drift step**, `.github/workflows/ci.yml:309-322`. It carries no
`if:`, so it runs on every CI run, and it is `moon run contracts:generate` followed by
`git diff --exit-code` over the three generated dirs. A reversed order leaves
`error_details_pb.ts` deleted from the working tree, and `git diff` reports the deletion against the
index. A `@paigasus/proto` test asserting `ErrorInfoSchema.typeName === 'google.rpc.ErrorInfo'` is
kept as a fast local signal, but it is not the control and this spec no longer claims it is.

**Measured, and corrected here: the residual is real but narrower than a prior draft of this
section stated.** `CONTRACTS_GENERATE_INPUTS` (`ci_targets.py:393-400`) pins `contracts:generate`'s
*inputs*; `check_contracts_generate_inputs` (`ci_targets.py:1773-1786`) reads `inputGlobs` and
`inputFiles` and nothing else, and no gate asserts the task's `script:` *content*. But Moon's task
hash DOES include the script — `.moon/cache/hashes/<hash>.json` carries a `script` key holding the
block verbatim — so any script edit, accidental or not, is a cache miss: `buf` re-runs, and the
unconditional codegen-drift step catches a reversed order (the googleapis file deleted) or a dropped
`--path` (extra generated files) through its `git diff`. What is unpinned is narrower: no gate
asserts the script says what it should say, so a human could still change the ordering, drop
`set -euo pipefail`, or narrow `--path` further, and ship it deliberately with the drift step
passing on the new, self-consistent output. Adding a script-content pin means a new registry
obligation and is **out of scope** (§ 14); this is recorded as a stated limitation, in the repo's
own habit of naming a residual rather than implying it is closed.

**The drift step has a known vacuity.** On a Moon task-cache hit `buf generate` never re-runs and
the diff compares the committed output against itself — the hole SMA-592 closed for the generator
pins. That is exactly why `contracts/buf.gen.googleapis.yaml` must join `contracts:generate`'s
inputs, and hence `CONTRACTS_GENERATE_INPUTS` (§ 11.2).

### 5.2 The BSR network dependency is not new

**MEASURED (M9).** `buf.gen.yaml` already declares three `remote:` plugins (lines 8, 30, 39). A
`remote:` plugin executes server-side on the BSR, so every `buf generate` already makes a live
network call, and `~/.cache/buf/v3/plugins` is **empty** — remote plugin execution is not cached
locally. `ci.yml` caches cargo, `.moon/cache`, the pnpm store and uv (`:81-120`, `:155-169`), and
nothing buf-related. Separately, `~/.cache/buf/v3/modules/b5/buf.build/googleapis/googleapis/`
is already populated from `buf.yaml`'s existing `deps:` resolution.

So the second invocation adds one more BSR round-trip to a step that already cannot run offline. It
does not introduce a new class of dependency, and a BSR outage already reds every CI run today.

**The vendored alternative is therefore rejected**, and the reason is recorded rather than left
implicit: committing `error_details_pb.ts` by hand would remove the ordering constraint and the
`CONTRACTS_GENERATE_INPUTS` edit, but it would put a wire contract outside the codegen-drift gate —
the one thing ADR-0019 built the registry to avoid — to buy back an offline capability the repo does
not have and does not claim.

### 5.3 Recovery from a phase-2 failure

`clean: true` wipes the tree before phase 1. If phase 2 fails, `set -euo pipefail` aborts with
`google/rpc/error_details_pb.ts` deleted from the working tree, and later tasks in the same
`moon ci` read that tree. The recovery is `moon run contracts:generate --force`; **do not commit the
deletion**. This is written into `contracts/moon.yml`'s comment beside the script, because the
failure presents as an unrelated multi-task failure and the recovery is not obvious.

Generating into a scratch directory and moving on success would remove the window. It is not done
here because it makes the task's `outputs:` and the drift step's `git diff` disagree about where
files live, and the window is a transient-BSR-failure window on a step that already cannot run
offline (§ 5.2). Stated as a deliberate trade, not an oversight.

## 6. Package shape

### 6.1 Five entry points

```json
"exports": {
  ".":              "./src/index.ts",
  "./iam":          "./src/iam.ts",
  "./chat":         "./src/chat.ts",
  "./errors":       "./src/errors/map-error.ts",
  "./errors/types": "./src/errors/types.ts"
}
```

Subpaths exist so a caller that only maps an error does not pull `@connectrpc/connect-node` and its
HTTP/2 stack into its module graph.

### 6.2 `server-only` — AC 1

`src/server-guard.ts` holds the single `import 'server-only';` and nothing else. One site, so a
future entry that forgets it is a visible omission rather than a copied line that drifted.

Four layers, and the honest limits of each:

1. **`import 'server-only'`.** Its exports map is `{ "react-server": "./empty.js", "default":
   "./index.js" }` and `index.js` is one unconditional `throw`. A client component that takes a
   **value** import of a guarded entry fails the build.
2. **The eslint boundary rule**, already live and already naming this package
   (`ts/packages/paigasus-next-config/src/eslint.mjs:91-104`): the SDK may import `@paigasus/proto`
   and nothing else in the `@paigasus/*` namespace, and `react`/`react-dom`/`next` are banned. This
   is why the SDK cannot import `@paigasus/auth` (§ 14) and cannot reach `next/headers`. **It
   covers static imports only** — `no-restricted-imports` does not see a dynamic
   `import('next/headers')`.
3. **A structural test** asserting every guarded entry named in the `exports` map imports the guard
   as its **first import statement** (not its first line — every file opens with the SPDX header),
   resolving the path relative to each entry. Driven off `package.json`, so a sixth entry point is
   covered the day it is added.
4. **`tsconfig.json` excludes DOM types** (§ 6.4).

**AC 1 is enforced structurally and is not proven by a build.** No test in this package runs a
client-side Next build, so no test observes the throw. The sibling package has the same shape and
the same gap (`ts/packages/paigasus-next-config/tests/runtime.test.ts:63-68` covers the edge runtime,
not the client bundle). A console-side failing-build fixture would prove it and is deliberately left
to SMA-510, which will have a console consuming the SDK. Stated plainly rather than implied, in the
same spirit as § 10's live-service paragraph.

`server-only` guards the **client bundle only**. Next sets the `react-server` condition for the
middleware layer too, so it resolves to `empty.js` there and is a no-op — the trap SMA-502 measured.
The SDK is not middleware-reachable by design and reads no environment, so no `NEXT_RUNTIME` check is
added.

### 6.3 Types cross the boundary; values do not

Revision 1 made every entry server-only while also saying `presentation` is what a client error
boundary switches on. Those contradict: a Next `error.tsx` is `'use client'`.

The resolution rests on a configuration fact. `ts/tsconfig.base.json:9` sets
`verbatimModuleSyntax: true`, so `import type` emits **nothing** and a value import of a type is a
compile error. A client component can therefore write

```ts
import type { PaigasusError, Presentation } from '@paigasus/sdk/errors/types';
```

with no runtime import and no `server-only` evaluation.

`errors/types.ts` carries **no guard**, because it holds only types and its `./errors/types` subpath
exists to make the client-safe surface explicit rather than a subtlety about erasure. `mapError`
stays behind the guarded `./errors`: it runs in the BFF, and the client receives an already-mapped
`PaigasusError` as a serializable prop. That is also what AC 4's "raw gRPC statuses never reach the
browser" requires — `PaigasusError` holds no `ConnectError` and no `Headers`.

M10 measures that a client component `import type`-ing from the **guarded** `./errors` entry also
builds, which is the belt-and-braces case.

### 6.4 `tsconfig.json` needs `types: ["node"]`

`ts/tsconfig.base.json:6` sets `lib: ["ES2022"]` and no `types`. § 8 uses `fetch`, `Response` and
`ReadableStream`; § 9.4 takes `Headers`. None exists under that configuration. The package therefore
sets `"types": ["node"]` and adds `@types/node` as a devDependency, matching
`ts/packages/paigasus-next-config/tsconfig.json:4-9`.

§ 6.2 layer 4 bans **DOM** types specifically. It does not ban node types, which are required.

### 6.5 vitest

`environment: 'node'`, `include: ['tests/**/*.test.ts']`, and **both** `resolve.conditions` and
`ssr.resolve.conditions` set to `['react-server', 'node', 'import', 'default']`. Vitest 5 resolves a
node-environment test's imports through `ssr.resolve.conditions`, not the top-level key — measured on
SMA-502. Listing `react-server` alone would drop `import`/`default` and break source-exports `.ts`
resolution for `@paigasus/proto`.

`vitest.config.ts` matches neither `src/**/*` nor `tests/**/*`, so it must be **appended** to the
`test` task's inputs or an edit to this block serves a cached PASS — the reason
`ts/packages/paigasus-ui/moon.yml:14-17` gives for the identical line.

## 7. Transport, caching and per-call auth

ADR-0018 decision 6: *"Transports are cached per base URL; authorization is attached per call via an
interceptor reading the request-scoped session. No token ever lives in a cached object."*

The interceptor lives on the transport and the transport is cached, so the interceptor cannot close
over a token. Connect-ES v2's `ContextValues` is the mechanism (**MEASURED, M2**):
`CallOptions.contextValues?: ContextValues` (`call-options.d.ts:30-34`) and the interceptor request
carries `readonly contextValues: ContextValues` (`interceptor.d.ts:143`).

### 7.1 The cache key is the full transport identity, not the base URL

`createGrpcTransport` also takes `nodeOptions` (TLS trust material), `defaultTimeoutMs` and
`httpVersion`. Keying on the base URL alone would let two different trust configurations share one
transport — and this repo already carries four CA-bundle knobs with divergent semantics, so a second
option is a question of when, not whether.

The key is therefore a stable serialization of the **whole options object**, and the factory takes
that object rather than a bare string. Today it holds one field; the rule is written down now so
adding the second does not silently alias two transports.

This is also why `Auth` (§ 7.4) is a parameter of the client factory and never of `getTransport`: the
transport identity — and therefore this cache key — must stay token-free.

### 7.2 A default deadline is mandatory

`defaultTimeoutMs` is set — **10 s** — rather than left unset. An unset deadline lets a gRPC call in
a Next server component hang the request forever, which is a production hazard and not a default
worth inheriting. Callers override per call via `CallOptions.timeoutMs`.

### 7.3 Nothing evicts, and that is stated

The cache is a module-level `Map` that never evicts and never closes an HTTP/2 session. Under a
Next server this is correct — a handful of long-lived transports to a fixed set of in-cluster
services is the intended shape. It has one consequence worth naming: an open HTTP/2 session keeps a
Node process alive, so the package exports `disposeTransports()` and the vitest suite calls it in
`afterAll`. Without it `vitest run` can hang after the assertions pass.

### 7.4 A forgotten token is a type error, not a 401

Revision 1 read the token from `contextValues` with a `null` default, so a caller that forgot it
sent an unauthenticated request and got a runtime 401 — the most likely mistake, in a package whose
stated purpose is attaching bearer tokens.

Instead the client factory takes the token and binds it into the per-call `contextValues`:

```ts
export function createIamClient<S extends DescService>(
  service: S,
  opts: TransportOptions,
  auth: Auth,                     // required — omission is a compile error
): Client<S>;

type Auth = { bearer: string } | { anonymous: true };  // opting out is explicit
```

The token is a parameter to the **client**, never to `getTransport`, so it is still absent from the
cached object. `Auth` is a union rather than an optional so that an unauthenticated call — the health
check is the real case — is a written decision rather than an omission.

### 7.5 A client is request-scoped; the transport is not

The transport is cached and shared deliberately (§ 7.1–§ 7.3). The **client** returned by
`createIamClient` is not cacheable: it holds a bearer, so its lifetime is one request.

A module-scope `const client = createIamClient(...)` is a cross-request token leak — the same leak
§ 7 exists to prevent, moved one level up from the transport to the client. The rule is stated here
because the type system cannot express it: nothing about `Client<S>`'s shape marks it request-scoped.

The test tier gains a **cross-request token-isolation test** (§ 10): two clients built from the same
cached transport with different `Auth` values must produce two different `authorization` headers, and
neither request may carry the other's token.

## 8. The chat client

`POST /v1/chat/completions` on the gateway, the one hand-written surface (§ 4.1).

### 8.1 Four response branches, not two

Revision 1 described two branches. The gateway has four, and the difference decides what `mapError`
must tolerate. **MEASURED (M12)** against `rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs`:

| Case | Status | `content-type` | Body |
|---|---|---|---|
| Non-stream, upstream answered | upstream's | `application/json` (`chat.rs:118`) | **the upstream body, verbatim** |
| `stream:true`, upstream 2xx | upstream's | `text/event-stream` (`chat.rs:137`) | upstream SSE frames, unbuffered, plus the terminal adapter |
| `stream:true`, upstream non-2xx | upstream's | `application/json` (`chat.rs:141`) | **the upstream body, verbatim** |
| The gateway's own failure | its own | `application/json` (`error.rs:203`) | `{error:{message,type,param,code}}`, `code` from the registry |

Two consequences Revision 1 missed, both load-bearing:

- **A non-2xx chat body is usually not the gateway's envelope.** The module doc says it in its own
  words (`chat.rs:19-20`): *"Forward the upstream response verbatim … A non-2xx upstream (OpenAI's
  own error envelope) passes through unchanged."* The registry assertion Revision 1 cited
  (`error.rs:296-309`) enumerates `GatewayError` only, so it says nothing about a passthrough body.
  The gateway's own envelope appears only for the failures it raises itself — missing/invalid
  credential, denied authz, missing scope, bad body, invalid schema, too large, upstream
  unavailable, upstream timeout, streaming disabled.
- **`content-type` is forced, not observed.** All three non-SSE branches hardcode
  `application/json` regardless of what the upstream sent. An upstream HTML 502 page arrives
  labelled JSON. So the mapper must tolerate a body that does not parse.

### 8.2 The request and response contract

Revision 1 wrote the return type as `{ kind: 'json', … } | { kind: 'stream', body }` and never
filled in the ellipsis, never named the request type, and never said whether a non-2xx throws or
returns. All three are settled here.

```ts
interface ChatCompletionRequest {
  model: string;                  // required by the gateway (dto.rs:22-34)
  messages: unknown[];            // required; shape is OpenAI's, not ours to pin
  stream?: boolean;
  [key: string]: unknown;         // the gateway preserves unknown fields (#[serde(flatten)])
}

// The ids sit on the BASE, so both variants carry them. A caller logging a
// successful non-streaming completion has an id to log, exactly as a streaming
// caller does. The gateway's CorrelationLayer stamps `paigasus-correlation-id`
// and `paigasus-request-id` on EVERY response (correlation.rs:174-175), success
// included, so there is nothing to measure here — the headers are always there.
interface ChatResultBase {
  status: number;
  correlationId: string | null;
  requestId: string | null;
}

type ChatCompletionResult =
  | (ChatResultBase & { kind: 'json'; body: unknown })
  | (ChatResultBase & { kind: 'stream'; body: ReadableStream<Uint8Array> });

function chatCompletion(
  request: ChatCompletionRequest,
  options: ChatOptions,
): Promise<ChatCompletionResult>;
```

- **`model` and `messages` are required.** A body missing either is `Category::Data` at
  `chat.rs:93`, rendered as a 400 `invalid-request-schema`. Making them required in the type turns a
  round-trip into a compile error, the same principle § 7.4 applies to the bearer token.
- **`messages` is `unknown[]`, deliberately.** The gateway does not model message content, and
  pinning OpenAI's message union here would make every upstream addition a breaking change in this
  package.
- **Branch on `content-type`, never on the caller's own `stream` flag.** Row 3 of § 8.1 is a
  `stream: true` request answered as JSON. A client that trusts its own flag misreads it.
- **A non-2xx throws a `PaigasusHttpError`, which CARRIES the `PaigasusError`.** One rule, not two:
  the result union covers 2xx only, and every failure — transport, non-2xx, or a body that will not
  parse — leaves as a throw. A third `{ kind: 'error' }` variant would let a caller ignore a failure
  by not checking a discriminant.

  ```ts
  export class PaigasusHttpError extends Error {
    readonly error: PaigasusError;
  }
  ```

  **Amended after implementation review.** Revision 2 said the client throws the `PaigasusError`
  itself. That trips `@typescript-eslint/only-throw-error`, and the rule is right: a plain data
  object has no stack, so `catch (e) { logger.error(e) }` in BFF middleware yields a log line with
  no origin, and both `pino`'s `err` serializer and Next's error boundary test `instanceof Error`.
  The two concerns are separable — the thrown value needs a stack, the *rendered* value needs to be
  a serializable prop — so the class carries the data object rather than replacing it. § 9.1's shape
  is unchanged and still crosses the RSC boundary. This also aligns with `@paigasus/auth`, whose
  `core/errors.ts` already bases every failure on `abstract class AuthError extends Error`; two
  packages in one SDK should not disagree on how a failure is thrown. Deferring it would make it a
  breaking change the moment a console consumes the package.
- **The streaming variant carries the ids** because the terminal frame cannot (§ 8.4).

### 8.3 Streaming is a passthrough — AC 4

The upstream `Response.body` is returned as-is. The SDK does not read, buffer, decode or re-encode
it, and the object the caller receives is the identical `ReadableStream`. The gateway forwards
upstream SSE chunks unbuffered (`chat.rs:128-137`); buffering here would undo that.

**There is no idle timeout, and that is a decision, not an omission.** Revision 1's § 8.1 asked for
one. It cannot coexist with this section: detecting "no bytes for N ms" needs an observer in the
byte path, and every mechanism that provides one — `pipeThrough(new TransformStream(…))` or a
self-driven reader — returns a **new** stream and makes the SDK a reader. The `TransformStream`
variant carries a second defect: `transform()` runs only when the consumer pulls, so a slow caller
produces no chunk events and the timer fires against a healthy upstream, aborting exactly the long
completions the streaming surface exists to serve.

Stall detection therefore belongs to the caller that owns the reader, which is the only place with
the information to do it correctly. Stated here as a non-goal so a reader does not mistake it for
an oversight.

### 8.4 Deadlines and cancellation

- **A pre-header deadline of 10 s**, matching § 7.2's gRPC transport so the two surfaces agree. The
  mechanism is an `AbortController` plus a timer, merged with the caller's optional signal through
  `AbortSignal.any([caller, deadline])`, and **the timer is cleared the moment `await fetch()`
  resolves**. A single signal held past that point would abort the body too, which is why one
  `AbortSignal` cannot serve as both a header deadline and a live-stream lifeline.
- **After headers, no wall-clock timeout and no idle timeout apply** (§ 8.3). A long chat completion
  is a correct slow response.
- **"Abandoning" the stream means calling `cancel()` on it.** Dropping the reference is not
  observable — no hook exists — so a requirement written against it would be unimplementable. On an
  explicit `cancel()`, `fetch` already propagates cancellation to the upstream connection; the
  passthrough is what preserves that, and the SDK adds nothing of its own.

**Coverage limit, stated rather than implied.** § 10's suite drives a stub `fetch`. A test asserting
the stub's own `cancel` was called proves the SDK forwarded the call, not that a socket closed.
Proving the latter needs a loopback `node:http` server, which this package does not stand up. The
row is scoped to the forwarding claim and § 10 says so.

### 8.5 The terminal SSE frame

A failure **after** the head is committed cannot change the status, so the gateway injects exactly
one terminal SSE frame and ends the stream. Verbatim, from `chat.rs:63`:

```
data: {"error":{"message":"upstream stream error","type":"api_error","param":null,"code":"upstream-error"}}\n\n
```

**Mid-stream errors are not surfaced by this client.** Scanning the stream means buffering, which
defeats § 8.3. The parser is therefore **exported** for a caller consuming its own stream.

A `ReadableStream` chunk is not guaranteed to hold one complete SSE record. A record is delimited by
a blank line and a chunk boundary can fall anywhere, so a parser over one chunk is lossy precisely
on the split-frame case — the terminal error split across two chunks, the one case it exists to
catch. The exported shape is a stateful incremental parser:

```ts
export function createTerminalFrameParser(
  ids: FrameIds,
  committedStatus: number,
): {
  push(chunk: Uint8Array | string): PaigasusError | null;
};
```

Six points of the contract, each closing a way the parser could silently miss the frame:

1. **It takes the ids and the committed status.** The frame arrives inside a committed 2xx whose
   headers the chat client already read, and those headers carry `paigasus-correlation-id` and
   `paigasus-request-id`. The frame itself carries neither id nor a status the parser could observe
   for itself — it sees only bytes. The caller supplies the committed 2xx status from the streaming
   result, which is what lets a non-200 2xx such as `201` be preserved in
   `PaigasusError.transport.status` rather than the parser assuming `200`. Without the ids argument,
   the hardest chat failure to support is the one error with no reportable id.
2. **It accepts `Uint8Array`** and owns a single streaming `TextDecoder` (`{ stream: true }`).
   Decoding each chunk with a fresh decoder corrupts a multi-byte character split across a chunk
   boundary — the same defect class the split-record argument above identifies, one level down. A
   `string` is accepted too, for a caller that already decoded correctly.
3. **The buffer is capped** at 64 KiB. "Holds only the trailing partial record" is true only if a
   blank line eventually arrives; an upstream that never sends one grows it without bound. On
   overflow the parser drops the buffer and returns `null` — it is a best-effort observer, never a
   reason to fail a stream that is still delivering data.
4. **`push` returns the first terminal error, then `null` forever.** Revision 1's doc comment said
   it "yields each complete SSE record it can form" while its signature returned one nullable value.
   The signature is right and the comment was wrong: the frame is terminal, so there is nothing
   after it worth reporting. Records before it are ordinary data and yield `null`.
5. **A record ends at a blank line — any two consecutive line terminators, each CRLF, LF or CR.**
   `chat.rs:63` emits `\n\n`, but upstream chunks pass through verbatim, so the parser follows the
   SSE grammar rather than enumerating that one producer's delimiter.
6. **It maps through § 9.5 arm 4**, so a terminal frame and a non-2xx body produce the same shape.

### 8.6 The drift check, and what makes it run

The parser's test is driven by a fixture holding the exact frame above, plus a check asserting the
fixture still matches the Rust constant. Without the fixture the test would hand-build the object it
expects and could not fail when the gateway's frame changes.

**A fixture check that never runs is worse than none, and Revision 1 specified exactly that.** The
SDK's `test` task keys on `@group(sources)`, `@group(tests)`, `package.json`, `/ts/pnpm-lock.yaml`,
`/ts/packages/paigasus-proto/src/**/*` and `vitest.config.ts`. No `rs/**` path is among them, so
editing `chat.rs:63` selects no TypeScript task and the drift test never executes on the one PR it
exists to catch. This is the identical defect § 11.1 measured (M11) and fixed for AC 3.

The fix is **not** a new `repo:*` gate — that would carry five to seven registration obligations for
a single file comparison, and § 11's "this package adds no `repo:*` gate" stays true. Two edits,
mirroring the `proto->sdk` pattern exactly:

1. `paigasus-sdk-ts:test` gains the input
   `/rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs`, appended.
2. A new `run_task_case_ci "chat-rs->sdk"` case in `ci/affected-graph/run.sh`, anchored on that
   path. It is **the only control** on obligation 1 — remove the input and nothing else reds. The
   expected set is **derived by measurement, never transcribed**, the same rule § 11.2 states for
   `contracts->proto`.

Anchoring on a single named file rather than a glob is why one case suffices here: the two-anchor
rule the `proto->sdk` / `proto-iam->sdk` and `ui->console` / `ui-components->console` pairs follow
exists to prove a **glob's width**, and a literal path has none.

## 9. The error model

### 9.1 One shape

```ts
interface PaigasusError {
  presentation: Presentation;
  domain: ErrorDomain | null;
  reason: ErrorReason | null;              // null === unmapped
  rawReason: string | null;                // what the wire actually said
  rawDomain: string | null;                // what the wire actually said, for domain
  message: string;                         // human-readable. NEVER branched on.
  correlationId: string | null;
  requestId: string | null;
  retryable: boolean | null;               // null === the wire's "unknown"
  metadata: Readonly<Record<string, string>>;
  transport: { kind: 'grpc'; code: Code } | { kind: 'http'; status: number };
}

type Presentation =
  | 'relogin' | 'forbidden' | 'not-found' | 'degraded' | 'rate-limited'
  | 'invalid-input' | 'conflict' | 'disabled' | 'generic';
```

`message` is carried for display and logging and is never an input to a branch. AC 2 is satisfied
structurally: no function in `src/errors/` reads `message`.

**`domain` is `null` on three of the four arms, structurally.** IAM's HTTP envelope is exactly
`{error:{code,message}}` (`iam/.../http/error.rs:35`) and the gateway's is
`{message,type,param,code}` (`gateway/.../http/error.rs:40-45`) — neither has a domain field, and
the generated enum's own comment records it: *"Not emitted on the gateway's OpenAI-compatible
envelope, which has no domain field."* So ADR-0019 decision 9's `(domain, reason)` pair is available
on the **gRPC arm alone**. This is the primary reason § 9.4's table is keyed on `reason` — three
arms cannot supply the pair at all — and it is stated here because § 1 invokes the pair as the
model's foundation.

`rawDomain` exists for the same reason as `rawReason`: a newer service sending an unregistered
domain would otherwise lose the wire value.

`retryable` is tri-state deliberately. The wire's values are exactly `"true" | "false" | "unknown"`
(`correlation.rs:59-65`); **anything else maps to `null`, never to `false`**. Collapsing `unknown`
to `false` would assert a non-retryability the service declined to assert. ADR-0019 decision 7
exists so clients stop inferring.

**`null`, not `undefined`, at every optional field.** The codec `@paigasus/proto` ships returns
`undefined` (`error.ts:55,80`), so every call site normalizes with `?? null`. `PaigasusError` is
serialized to a client component as a prop, and `undefined` does not survive that boundary uniformly
while `null` does.

`transport` carries the raw code for logging. It is a number or a `Code`, never a `ConnectError` and
never `Headers`, so AC 4's "raw gRPC statuses never reach the browser" holds when the whole object
is serialized.

### 9.2 Two total mappings from transport status

Both tables are total by falling through to `generic`:

| gRPC `Code` | Presentation | | HTTP status | Presentation |
|---|---|---|---|---|
| `Unauthenticated` | `relogin` | | 401 | `relogin` |
| `PermissionDenied` | `forbidden` | | 403 | `forbidden` |
| `NotFound` | `not-found` | | 404 | `not-found` |
| `Unavailable` | `degraded` | | 502, 503, 504 | `degraded` |
| `DeadlineExceeded` | `degraded` | | 408 | `degraded` |
| `InvalidArgument` | `invalid-input` | | 400, 413, 415, 422 | `invalid-input` |
| `AlreadyExists`, `FailedPrecondition`, `Aborted` | `conflict` | | 409 | `conflict` |
| `ResourceExhausted` | `rate-limited` | | 429 | `rate-limited` |
| everything else, `Unimplemented` included | `generic` | | everything else | `generic` |

**`Unimplemented` maps to `generic`, and that is the change that makes § 9.4 real.** Revision 1
mapped it to `disabled`, which made the `CAPABILITY_DISABLED` override a no-op — the table's own
justification, defeated by the table above it. `Unimplemented` means "this build cannot do that",
which is a `generic` failure; "this deployment turned that capability off" is a different screen,
and § 9.4's entry is what selects it. There is no `501 -> disabled` row for the same reason: no
service in this repo emits 501, so the row was unreachable.

**429 has its own state, and Revision 1's `degraded` was wrong.** "Try again in a moment, you are
being rate-limited" and "the service is unwell" are different screens, and lumping them cost the
first the only copy that helps a user act. 504 stays under `degraded`: a timeout is a service
problem, not a quota one.

**MEASURED (M13): nothing in this repo emits 429 or `ResourceExhausted` today.** No `GatewayError`
variant renders 429 (`gateway/.../http/error.rs:120-177`), `convert.rs` maps no `ErrorClass` to
`ResourceExhausted`, and `error.proto` declares no quota or rate-limit reason at all. So every 429
the SDK can currently see arrives through the gateway's **upstream passthrough** (§ 8.1) and means
one thing: OpenAI's quota. `rate-limited` therefore has exactly one meaning today, and the copy can
say so plainly.

**When Paigasus gains its own quota, § 9.4 is the mechanism for its distinct copy** — not a second
`Presentation` value. A Paigasus quota needs a registry reason to exist at all; that reason takes an
override entry, and the entry selects whatever screen it deserves. An upstream 429 stays
`rate-limited` because its `code` is OpenAI's and `fromWireReason` rejects it, so it falls through
to this table. The two are already distinguishable, by `reason` being `null` or not. Adding a
second presentation value now would be an unreachable row — the defect this revision removed from
the `501 -> disabled` row.

The `ResourceExhausted` row is listed although unreachable today: the gRPC table enumerates `Code`,
and this is the right answer if IAM ever adds a quota. Stated so the row is not mistaken for
evidence that something emits it.

413 sits under `invalid-input` because `RequestTooLarge` is caller-fixable — the caller must send
less, which is the same corrective action a 400 asks for.

**`not-found` is unreachable on the chat path.** No `GatewayError` renders a 404, and axum's
unmatched-route 404 is plain text outside the envelope. The row serves the IAM arms.

### 9.3 The wire-reason codec lives in `@paigasus/proto`

**Shipped in PR A (SMA-624).** `ts/packages/paigasus-proto/src/error.ts` exports `asWireReason`,
`fromWireReason`, `asWireDomain` and `fromWireDomain`, all derived from the descriptor, with the
`^[a-z][a-z0-9]*(-[a-z0-9]+)*$` allow-list checked *before* any case transform (M7's Unicode folding
argument). `src/error.test.ts` already covers both directions over all 57 reasons and the ten
malformed inputs the Rust test rejects. **PR C consumes this and adds nothing to it** — § 10's
"wire-reason codec parity" tier is inherited, not re-implemented.

### 9.4 The presentation override table — AC 3

`src/errors/presentation.ts` holds

```ts
type Entry = Presentation | 'from-transport';
const PRESENTATION: Record<Exclude<ErrorReason, ErrorReason.UNSPECIFIED>, Entry> = { … };

// The lookup is a function, not a bare index. `fromWireReason` returns
// `ErrorReason | undefined` — a type that INCLUDES UNSPECIFIED even though the
// implementation excludes it at runtime (error.ts:63) — and no narrowing expresses
// "not UNSPECIFIED". Indexing PRESENTATION directly at the call site therefore does
// not compile. This is the one place that handles the sentinel.
export function presentationFor(reason: ErrorReason): Entry {
  return reason === ErrorReason.UNSPECIFIED ? 'from-transport' : PRESENTATION[reason];
}
```

**MEASURED (M8), on TypeScript 6.0.3:** omitting a member yields
`TS2741: Property '[ErrorReason.INTERNAL]' is missing`, naming it. `ErrorReason` is a real numeric
TS enum, so `Exclude` narrows it to the literal union of the other 57 members and the mapped type is
fully writable.

Two mechanisms, both required:

1. **The total `Record`** makes a missing reason a compile error.
2. **A table test** iterates `ErrorReasonSchema.values`, skips the sentinel, and asserts each reason
   round-trips and has an entry. The round-trip must guard the intermediate value —
   `fromWireReason(asWireReason(r))` does **not** compile, because `asWireReason` returns
   `string | undefined` and `fromWireReason` takes `string`:

   ```ts
   const wire = asWireReason(reason);
   expect(wire).toBeDefined();
   expect(fromWireReason(wire!)).toBe(reason);
   ```

The type alone can be switched off by a refactor to `Partial<…>`, and the test notices. The test
alone runs later. Both are kept.

**Two entries change an answer, and one pins an agreement.**

- **`CAPABILITY_DISABLED` (904) — load-bearing.** It reaches a client only as gRPC `Unimplemented`
  (`convert.rs:96-104`), which § 9.2 now maps to `generic`. The entry lifts it to `disabled`. The
  capability name rides in `metadata["capability"]`, so the screen can name what is switched off.
- **`PRINCIPAL_INACTIVE` (29) — load-bearing.** IAM maps it to `PermissionDenied` / 403
  (`convert.rs:144`, `authn.rs:42`), so § 9.2 alone renders a deactivated account as `forbidden` —
  "you do not have permission" for an account that exists and is switched off. The entry maps it to
  `disabled`. Its two neighbours, `IDENTITY_NOT_PROVISIONED` and `PROVISIONING_FAILED`, keep
  `forbidden`: those are provisioning states an administrator resolves, and `forbidden` reads
  correctly for them. Recorded as a considered split, not an oversight.
- **`INVALID_REQUEST_SCHEMA` (906) — a pin, not a change.** IAM answers **422** (axum's
  `JsonDataError` path), the gateway answers **400**, for the identical wire code;
  `error.proto:239-247` says so and warns that *"a consumer mapping code -> status must not assume
  it is one-to-one."* Both already land on `invalid-input` under § 9.2, so the entry pins that
  agreement: a future change to either status cannot silently split one code's presentation across
  two services. Called out as a pin so the table is not credited with a transform it does not make.

Every other reason takes the literal `'from-transport'`, so all 57 are a reviewed decision rather
than a default.

**The table is keyed on `reason`, not `(domain, reason)`** — primarily because three of the four
arms carry no domain at all (§ 9.1), and secondarily because the one reason whose status differs by
site resolves to the same presentation either way. If a divergence appears where the presentations
genuinely differ **and** it arrives on the gRPC arm, the key must become the pair.

### 9.5 Four inputs, one output

`mapError` accepts:

1. **A `ConnectError`.** Verified against the installed `@connectrpc/connect` 2.2.0 typings:
   `code: Code`, `metadata: Headers` (*"a union of response headers and trailers associated with
   this error"*), and `findDetails<Desc>(desc): MessageShape<Desc>[]` — an **array**, so
   `findDetails(ErrorInfoSchema)[0]` is `undefined` whenever no detail rode along. That is a
   reachable path — a network failure, a proxy, or a connection reset carries a status and no
   `ErrorInfo` — so arm 1 branches before reading it:
   - **Detail present:** `(reason, domain, metadata)`. `metadata` carries `retryable`, and
     `correlation_id`/`request_id` **when the error was raised inside a request scope**
     (`convert.rs:59-74`) — **omitted, not emptied**, outside one, which is why the fallback below
     is not decoration.
   - **Detail absent:** `domain`, `reason`, `rawReason`, `rawDomain` and `requestId` are `null`,
     `retryable` is `null` (the wire asserted nothing), `metadata` is empty, and `presentation`
     comes from the gRPC status table including its `generic` fall-through. `correlationId` is still
     read from `err.metadata`, since that is a union of headers and trailers and may carry it.
     Without this branch the SDK throws while mapping an error, turning a recoverable upstream
     failure into an unhandled exception in the BFF.
   - `message` uses `err.rawMessage`, not `err.message`: Connect-ES prefixes the latter with the
     status code (`[not found] …`), which is diagnostic noise in a UI string.
2. **An IAM HTTP response** — `{ status, headers, body }`, body `{error:{code,message}}`. The body
   carries **no** correlation id and **no** retryable; all three are headers
   (`paigasus-correlation-id`, `paigasus-request-id`, `paigasus-retryable`,
   `correlation.rs:20,31-33`). AC 2 asks for "a generic fallback plus correlation id", so the
   headers are not optional input.
3. **A gateway HTTP response** — `{ status, headers, body }`, the same three headers, and **two
   sub-cases the caller cannot distinguish in advance** (§ 8.1):
   - **The gateway's own envelope.** `{message,type,param,code}` with `code` drawn from the registry
     (`error.rs:296-309` asserts this over every `GatewayError`). `param` is `Option<String>`, so a
     JSON `null` is **omitted** from `metadata`, never stringified to `"null"`. `type` is not read:
     it takes two values and both are coarser than `code`.
   - **An upstream OpenAI body, forwarded verbatim.** Its `code` is OpenAI's vocabulary
     (`insufficient_quota`, `server_error`) or `null`. `fromWireReason` rejects the underscore form
     by its allow-list, so `reason` is `null`, `rawReason` keeps the wire value, and `presentation`
     falls back to the HTTP status table. **This is the ordinary path, not an edge case** — it is
     AC 2's degradation requirement doing its job on the SDK's most-used error surface.

   Arm 3 must therefore tolerate **a body that is not JSON at all** (the content-type is forced —
   § 8.1) and **a body that parses but carries no `error` object**. Both fall back to the status
   table with `rawReason: null`; neither throws. A mapper that throws while mapping is the defect
   arm 1's absent-detail branch exists to prevent, and arm 3 is far likelier to meet it.

   The gateway applies `CorrelationLayer` outermost (`gateway/.../http/mod.rs:111`), so the three
   headers are present on every response including the passthrough — which is the only retryability
   signal available when the body is OpenAI's.
4. **A parsed terminal SSE frame**, via § 8.5's parser. `transport` is
   `{ kind: 'http', status }` carrying **the committed 2xx status** — `chat.rs:128` branches on
   `status.is_success()`, so 200 is the usual but not the only reachable value. `retryable` is
   `null`: the frame deliberately carries no retryable signal, and no header can change after the
   head is committed (`chat.rs:56-62`). `correlationId` and `requestId` come from the parser's `ids`
   argument, read from the streaming response's own headers.

**Arm 2 has no in-SDK caller, and that is deliberate.** § 4.1 moves every IAM HTTP surface to gRPC,
so nothing in this package constructs arm 2's argument. It is kept as a **public helper** because
IAM's REST surface is the external customer-facing API (ADR-0018 consequence 6); an app calling it
directly should map its errors the same way. Its test drives it directly.

**`metadata` is the ErrorInfo map minus the three lifted keys.** `retryable`, `correlation_id` and
`request_id` are removed after being read into typed fields, so a caller cannot branch on a raw
duplicate that disagrees. `capability` and `field` survive.

**Unknown reason and domain handling (AC 2).** `rawReason` always holds what the wire said. When
`fromWireReason` rejects it, `reason` is `null`, `presentation` falls back to the transport-derived
value, and `correlationId` is preserved. An unrecognized code degrades to a generic presentation
plus a user-reportable id — never a throw, never an empty message. `rawDomain` is populated
verbatim whether or not `fromWireDomain` resolves it.

**The two system-retirement 409s are not special-cased.** They add sibling keys next to the standard
`error` object rather than replacing it (`system_retirement.rs:111-126`). `mapError` reads
`error.code` and ignores the siblings.

## 10. Testing

**Inherited, not re-implemented by PR C.** The wire-reason codec parity tier ships in
`@paigasus/proto`'s `src/error.test.ts` (PR A). The transport-cache and server-only-structure tiers
ship in `tests/transport.test.ts` and `tests/server-guard.test.ts` (PR B). PR C adds one edit to the
last of these — see the AC 1 row.

| Tier | What it proves |
|---|---|
| Presentation totality (AC 3) | Every descriptor value has an entry; `CAPABILITY_DISABLED` -> `disabled`, `PRINCIPAL_INACTIVE` -> `disabled`, `IDENTITY_NOT_PROVISIONED` -> `forbidden`, `INVALID_REQUEST_SCHEMA` -> `invalid-input` from **both** 400 and 422 |
| Transport-status tables (§ 9.2) | Named `(input, output)` pairs, including `Unimplemented` -> `generic`, `429` -> `rate-limited` (**not** `degraded`), `504` -> `degraded`, and both `generic` fall-throughs |
| AC 4 mapping | The four codes AC 3 names: `UNAUTHENTICATED` -> `relogin`, `PERMISSION_DENIED` -> `forbidden`, `NOT_FOUND` -> `not-found`, `UNAVAILABLE` -> `degraded` |
| `mapError` — gRPC | A `ConnectError` with a real `ErrorInfo` detail round-trips; `correlation_id`/`retryable` read from `metadata`; the three lifted keys absent from `metadata`; a `ConnectError` with **no** detail falls back to the status table; `rawMessage` used, so no `[code]` prefix leaks |
| `mapError` — IAM HTTP | Correlation id, request id and retryable from **headers**; tri-state retryable; an unexpected retryable value yields `null` |
| `mapError` — gateway HTTP | The gateway's own envelope; **an upstream OpenAI body** (`insufficient_quota`) degrading to `reason: null` with `rawReason` kept; a `null` `param` omitted from `metadata`; **a non-JSON body** and **a JSON body with no `error` object**, both falling back to the status table without throwing |
| `mapError` — degradation (AC 2) | Unknown reason yields `reason: null`, keeps `rawReason` and `correlationId`; unknown domain yields `domain: null`, keeps `rawDomain` |
| Message-independence (AC 2) | One wire error with three different `message` strings maps to three identical objects modulo `message` |
| AC 1 structure (edit) | `tests/server-guard.test.ts` computes each entry's expected guard specifier **from that entry's own directory**, so a nested entry resolves `'../server-guard.js'`. The shipped literal `"import './server-guard.js';"` cannot express this and is replaced |
| Chat — shape | Non-stream 2xx yields `{kind:'json'}`; a `stream:true` request answered as JSON (§ 8.1 row 3) is detected by `content-type`, not the flag; a non-2xx **throws** a `PaigasusHttpError` carrying the mapped `PaigasusError` on its `.error` property (§ 8.2) |
| Chat — ids on success | **Both** variants carry `correlationId` and `requestId` read from the response headers, on a 2xx — the non-streaming variant included. A response missing the headers yields `null`, not a throw |
| Chat — an upstream 429 | A passthrough 429 (OpenAI's quota) throws a `PaigasusHttpError` whose `.error` holds `presentation: 'rate-limited'`, `reason: null` and `rawReason` set to OpenAI's own code |
| Chat — passthrough (AC 4) | The returned `body` is the **identical** `ReadableStream` object the stub `fetch` produced |
| Chat — deadline | A stalled-header request rejects at the pre-header deadline; a response whose headers arrive in time is **not** aborted while its body is still streaming (the cleared-timer proof) |
| Chat — cancellation | `cancel()` on the returned stream reaches the stub's own cancel. **Scoped to forwarding** — see § 8.4's coverage limit |
| Terminal-frame parser | The pinned fixture; a frame split across two chunks; a data frame followed by the terminal frame in one chunk; a partial trailing record that never completes; a multi-byte character split across a chunk boundary; the buffer cap; `null` forever after the first terminal error; all three delimiters |
| Terminal-frame drift | The fixture still matches `chat.rs:63`, **and** the § 8.6 input plus affected-graph case make that test run when `chat.rs` changes |

**Three rows carry less than they appear to, stated rather than implied.**

*Presentation totality cannot fail while the total `Record` compiles.* It is a guard against a future
refactor to `Partial<…>`, not independent coverage. § 9.4 says this; § 10 repeats it so the row is
not read as proving more.

*No live-service tier.* The SDK has no server to talk to in CI. Transport behaviour is tested through
interceptors and a stub `fetch`. This suite proves the SDK *forms* correct requests and *interprets*
correct responses; it does not prove IAM or the gateway accepts them. Standing a service up is
SMA-509/SMA-510's integration surface. The chat cancellation row inherits this limit (§ 8.4).

*AC 1 is not proven by a build.* See § 6.2.

## 11. Registration obligations

Eight edits. Revision 1 listed four and missed the three that matter most.

### 11.1 The SDK's task inputs — this is what makes AC 3 real

**MEASURED (M11).** Editing `ts/packages/paigasus-proto/src/generated/paigasus/common/v1/error_pb.ts`
today selects `paigasus-proto-ts:{build,test,typecheck}`, `ts:lint`, `ts:fmt` and three `repo:*`
gates — and **no `paigasus-sdk-ts` task**. So both AC 3 mechanisms would run on a later, unrelated
PR, and an unmapped code would not fail a test. AC 3 would be vacuous.

`dependsOn` does not fix this: it schedules an upstream and never selects a downstream, as
`ts/apps/paigasus-console/moon.yml:7-10` says in the repo's own words. Task `inputs` are the only
thing that confers affectedness.

`ts/packages/paigasus-sdk/moon.yml` therefore carries **both**:

```yaml
dependsOn: ['paigasus-proto-ts']
tasks:
  build:     { inputs: ['/ts/packages/paigasus-proto/src/**/*'] }
  typecheck: { inputs: ['/ts/packages/paigasus-proto/src/**/*'] }
  test:      { inputs: ['/ts/packages/paigasus-proto/src/**/*', 'vitest.config.ts'] }
```

appended, never `options.merge: replace` — replacing would drop the four inherited inputs, the defect
SMA-503 fixed on the console.

**MEASURED (M11b):** with those inputs, the same edit selects `paigasus-sdk-ts:{build,test,typecheck}`.

### 11.2 The gates

1. **`ci/affected-graph/run.sh:258-259`** — the `contracts->proto` expected set gains
   `paigasus-sdk-ts`. Strict equality; re-measured (M4), not transcribed.
2. **A new `run_task_case_ci "proto->sdk"`** in the same file, anchored on
   `ts/packages/paigasus-proto/src/generated/paigasus/common/v1/error_pb.ts`. This is the only
   control on § 11.1's input list — without it, a future edit dropping that input leaves AC 3
   vacuous again and nothing reds. The `ui->console` pair (`run.sh:354-377`) is the precedent, and
   its comment says exactly why such a case is the control.
3. **`ci/affected-graph/ci_targets.py`** — the expected input set for `contracts:generate` exists
   in **THREE** places, and all three must move together:
   - the task's own `inputs:` in `contracts/moon.yml`;
   - `CONTRACTS_GENERATE_INPUTS` (`:393-400`), the strict-equality pin;
   - the `cg_ok` **self-test fixture** (`:3200-3207`), a frozen copy of the same set.

   The third was missed on the first attempt and reds CI on its own, with
   `ci-targets self-test FAILED: contracts:generate pin reported drift on the clean fixture`
   plus a cascading `negative-control FAILED` — `run.sh:437` runs the same self-test inside its
   negative control, so one defect prints two messages.

   **It cannot be caught locally by the obvious check.** `python3 ci_targets.py` (the real-run
   path) reads Moon's live output and passes, because the live inputs and the tuple agree. Only
   `--self-test` compares the tuple against the frozen fixture. Run **both** paths before pushing
   a change to this pin.

### 11.3 The packages

4. **`@paigasus/proto`'s public surface** — § 3 promised it and Revision 1 never specified it. The
   root barrel (`src/index.ts`) gains, through the existing single `"."` export:
   `ErrorReason`, `ErrorReasonSchema`, `ErrorDomain`, `ErrorDomainSchema`, `ErrorInfoSchema`
   (re-exported from the generated googleapis module), `asWireReason`, `fromWireReason`,
   `asWireDomain`, `fromWireDomain`. **Corrected during Task 3:** the seven `iam/v1` service
   descriptors — `TenancyService`, `AuthnService`, `AuthorizationService`, `ServiceAccountService`,
   `AuditService`, `UserService`, `OutboxService` — with their request/response types do **not**
   join the root barrel as this obligation originally said. `iam_pb.ts` retains a **deprecated**
   `ServiceInfo` message (`iam.proto:22-33`, kept only because buf forbids message deletion) whose
   runtime `ServiceInfoSchema` and type `ServiceInfo` both collide with the live
   `paigasus.common.v1` pair the root barrel already re-exports. **Measured, and corrected here:** a
   blanket `export *` does **not** error — the root barrel's explicit `ServiceInfoSchema` export
   shadows the star-exported iam/v1 one silently under the ES semantics TypeScript follows, so the
   root barrel would quietly serve the wrong `ServiceInfo` to anyone reaching for the iam one. That
   is a worse failure mode than a compile error, and it is the real argument for the split — not the
   duplicate-export error this section previously claimed. Hand-enumerating roughly a hundred message
   names to dodge the shadowing would need editing on every proto change. Instead they ship through a new curated `./iam`
   subpath (`src/iam.ts`, added to `package.json`'s `exports` map as `"./iam": "./src/iam.ts"`),
   which puts the two `ServiceInfo` names in separate modules that never collide.
   No `./generated/*` subpath is added, for `./iam` or for anything else: that would make the
   generated layout public API, so a codegen reshuffle would become a breaking change for
   consumers — `./iam` is a hand-written barrel file, not a passthrough, so this still holds.
5. **`ts/packages/paigasus-sdk/package.json`** — `dependencies`: `@paigasus/proto` (`workspace:*`),
   `@connectrpc/connect`, `@connectrpc/connect-node`, `@bufbuild/protobuf`, `server-only` (all
   `catalog:`). `devDependencies`: `typescript`, `vitest`, `@types/node`.
6. **`ts/pnpm-workspace.yaml`** — two new catalog entries, `@connectrpc/connect` and
   `@connectrpc/connect-node`, both `^2.2.0`, each with the header comment every existing entry
   carries. `@bufbuild/protobuf` (`^2.14.1`) and `server-only` (`^0.0.1`) are already catalogued;
   `@bufbuild/protobuf` satisfies connect-node's `^2.7.0` peer range.
7. **`contracts/buf.gen.googleapis.yaml`** — new file, TS-only, `clean: false`.
8. **`contracts/moon.yml`** — `generate` becomes a `script:` with `set -euo pipefail`, both
   invocations in order, plus `buf.gen.googleapis.yaml` in its `inputs`.

**Checked and not required:** none of `SELF_SCHEDULED_GATES`, `SELF_TASK_EXPECTED_GLOBS`,
`REQUIRED_REPO_TASKS`, `T_EXEMPT`, `cargo_moon_parity.py` or `task_inputs.py` holds a Moon project-id
list a new TS package perturbs — they are keyed on `repo:*` gate names and Rust crates. This package
adds no `repo:*` gate, so `ci.yml`'s `T=(…)` array and the CLAUDE.md marker command are untouched.
`repo:input-liveness` scans `repo:*` tasks only, so it does not see the SDK's inputs — which is
precisely why obligation 2 exists.

### 11.3a PR C's own obligations

Obligations 1–8 above belong to PRs A and B, which have merged. PR C carries **three** of its own,
and Revision 1 listed none of them.

9. **`ts/packages/paigasus-sdk/package.json`'s `exports` map** gains `"./chat"`, `"./errors"` and
   `"./errors/types"`. The file's own `_comment_entries` assigns this work here and states the rule
   PR B followed: *"An exports entry pointing at a stub is a public API promise this package would
   not keep."* So each key lands in the **same commit** as the file it names. `tests/server-guard.test.ts`
   is driven off this map, so the three entries are covered the day they are added — and
   `UNGUARDED_ENTRIES` already holds the exact string `'./errors/types'`, pre-registered by PR B.
10. **`paigasus-sdk-ts:test` gains the input**
    `/rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs`, appended (§ 8.6). Without it
    the terminal-frame drift check never runs on the PR that changes the frame.
11. **A `run_task_case_ci "chat-rs->sdk"` case** in `ci/affected-graph/run.sh`, anchored on that
    path — the only control on obligation 10. Expected set **derived, never transcribed**.

Obligation 9 also carries a correction to merged code: `tests/server-guard.test.ts` pins the literal
`"import './server-guard.js';"`, which a guarded entry in a subdirectory cannot satisfy. § 6.2
layer 3 always said the test "resolves the path relative to each entry"; the shipped test does not,
and PR C makes it so.

### 11.4 Concurrency with SMA-506

`run.sh:258-259` is a single-line CSV, so a concurrent branch editing it would collide. **SMA-506
does not.** Its § 14 is titled *"The package does not depend on `@paigasus/proto`"* and avoids the
edge deliberately, routing `RoleGrantRef` through locally-defined domain types. No collision exists.
Whichever branch merges second still rebases under `main`'s strict up-to-date rule and should
**re-measure** the expected set rather than transcribe it.

### 11.5 Dependency cooldown

`@connectrpc/connect` 2.2.0 published **2026-09-07**. pnpm 11's 24-hour `minimumReleaseAge` is
satisfied, but npm Dependabot has a 3-day cooldown. If CI reds on release age, that is the cause and
waiting is the fix.

## 12. Acceptance criteria mapping

| AC | Where |
|---|---|
| 1 — `import 'server-only'`; a client import fails the build | § 6.2 — four layers, **and a stated gap**: enforced structurally, not proven by a build |
| 2 — branch on `(domain, reason)` only; unknown → generic + correlation id | § 9.1, § 9.5; message-independence and degradation both tested |
| 3 — table test driven off the registry | § 9.4 — total `Record` (M8) **plus** a descriptor-driven test, made reachable by § 11.1 (M11) |
| 4 — four codes → four states; raw statuses never reach the browser | § 9.2 both tables; `PaigasusError` carries no `ConnectError` (§ 6.3, § 9.1) |
| 5 — `ci/affected-graph/run.sh` updated in the same change | § 4.2, § 11.2 — **corrected**: one id, measured (M4) |

## 13. Measurements

| # | Measurement | Status |
|---|---|---|
| M1 | A TS-only template against googleapis `--path google/rpc/error_details.proto` exits 0 and emits one self-contained file exporting `ErrorInfoSchema` | **TAKEN** |
| M2 | connect 2.2.0 exports `createContextKey`/`createContextValues`; `CallOptions.contextValues` and `req.contextValues` exist | **TAKEN** |
| M3 | `findDetails` requires a descriptor — no schema-free overload | **TAKEN** |
| M4 | `contracts->proto` with the edge is the seven existing ids plus `paigasus-sdk-ts`; no app enters | **TAKEN** |
| M7 | Node 24 folds `ı`→`I` and `ſ`→`S`, so the allow-list is load-bearing in TS as in Rust | **TAKEN** |
| M8 | `Record<Exclude<ErrorReason, UNSPECIFIED>, Entry>` yields TS2741 naming the missing member on tsc 6.0.3 | **TAKEN** |
| M9 | Every `buf generate` already contacts the BSR (three `remote:` plugins, empty plugin cache); `ci.yml` caches nothing buf-related | **TAKEN** |
| M11 | An `error_pb.ts` edit selects **no** `paigasus-sdk-ts` task today | **TAKEN** |
| M11b | With § 11.1's inputs it selects all three | **TAKEN** |
| M5 | Reversing the two `buf generate` calls leaves `error_details_pb.ts` deleted, and the **drift step** reds | **TAKEN** |
| M6 | Equal transport options return one object; differing options return two; two `Auth` values give two headers | pending |
| M10 | A client component `import type`-ing from the guarded `./errors` entry builds | pending |
| M12 | The gateway's chat endpoint has **four** response branches, and forwards a non-2xx upstream body verbatim under a forced `application/json` (`chat.rs:19-20`, `:113-119`, `:127-141`) | **TAKEN** |
| M13 | Nothing in this repo emits 429 or `ResourceExhausted`: no `GatewayError` renders 429, `convert.rs` maps no `ErrorClass` to it, and `error.proto` declares no quota reason | **TAKEN** |
| M14 | `ConnectError` 2.2.0 exposes `code: Code`, `metadata: Headers` and `findDetails<Desc>(desc): MessageShape<Desc>[]` — an **array**, so `[0]` is `undefined` when no detail rode along | **TAKEN** |
| M12 | Cold-`~/.cache/buf` cost of the second invocation in CI | pending |

M2, M3 and M8 were taken outside the workspace install tree — `npm pack` of the exact version, and a
standalone `tsc` invocation — because `@connectrpc` is not yet in `ts/pnpm-lock.yaml`. **They must be
re-confirmed once the catalog entries land**, since pnpm catalog resolution is what CI will use.
M1, M4, M9, M11 and M11b were taken in this worktree against the pinned toolchain.

## 14. Out of scope

- **Wiring any app to the SDK.** SMA-509 and SMA-510.
- **`IntrospectPrincipalResolver`.** SMA-506 § 14 names this issue as where the adapter lands, but the
  SDK's live boundary rule bans importing `@paigasus/auth`, and SMA-506 is unmerged. The SDK exports
  the typed `AuthnService` client and a mapper returning `{ scopePrn, roleKey }` objects; SMA-506's
  port is structurally typed, so the *app* wires them and neither package imports the other. Flagged
  for SMA-506's author rather than resolved unilaterally.
- **A script pin on `contracts:generate`'s ordering** (§ 5.1). It needs a new registry obligation.
  The residual is stated instead.
- **Widening `ci/error-registry/check.py` to TypeScript.** It is Rust-only and scoped to `rs/crates`.
  AC 3's descriptor-driven test covers the consumed side for this package; a repo-wide two-way TS gate
  is SMA-507's deferred AC 2.
- **A retry or backoff policy.** `retryable` is carried, not acted on.
- **grpc-web or any browser-direct transport.** ADR-0018 decision 2.

### 14.1 Recommended split into three PRs

The challenge argued this is two or three issues and the argument holds: the codegen change has the
widest blast radius in the repo — it touches a step that runs on every CI run — and is coupled to
none of the SDK work.

| PR | Contents | Why it stands alone |
|---|---|---|
| **A** | § 5 codegen, `@paigasus/proto`'s widened surface, the wire-reason codec (§ 9.3), obligations 3–4, 7–8 | Ships a tested contract surface with no SDK. Its blast radius is CI-wide, so it merges and settles first. |
| **B** | § 6 package shape, § 7 transport and auth, § 11.1 inputs, obligations 1–2, 5–6 | Needs A's exports. Adds the affected-graph edge and its control case. |
| **C** | § 8 chat, § 9 error model and tables | Needs A's `ErrorInfoSchema` and B's package. Holds AC 2/3/4. |

AC 5 lands in **B**, which is where the dependency edge is created. This is a recommendation for the
Linear breakdown, not a decision this spec takes.

## 15. What the adversarial challenge changed

**Folded in — three BLOCKERs.**

1. **AC 3 was vacuous.** The SDK's inherited task inputs never reach `@paigasus/proto`, so neither AC
   3 mechanism ran on the PR that adds a reason. Confirmed by measurement (M11) and fixed in § 11.1,
   with a new affected-graph case (obligation 2) as its control.
2. **`./errors` was server-only while `presentation` was called a client concern.** Resolved in
   § 6.3 by a `./errors/types` entry plus the `verbatimModuleSyntax` erasure fact.
3. **The widened `@paigasus/proto` surface was promised and never specified.** Now obligation 4.

**Folded in — MAJORs.** § 5.1's control corrected from a package test to the codegen-drift step, with
the ordering left explicitly unpinned; § 5.3's recovery note; § 6.4's `types: ["node"]`; § 6.5's
`vitest.config.ts` input; § 7.1's cache key; § 7.2's deadline; § 7.3's dispose path; § 7.4's
compile-time token; § 8's exported `parseTerminalFrame` plus fixture drift; § 9.2's HTTP table; § 9.4's
corrected `Record` type and its two real override examples; § 10's two stated coverage gaps; § 9.5's
arm-2 justification and metadata stripping; § 3.1's test-location convention; § 6.2 layer 3's "first
import statement" and layer 2's static-only caveat; § 13's measurement provenance.

**Rejected, with evidence.**

- *A merge collision with SMA-506 on `run.sh:258-259`.* SMA-506 § 14 deliberately avoids depending on
  `@paigasus/proto`. No collision (§ 11.4).
- *The second `buf generate` adds a new CI network dependency, so vendoring should be considered.*
  M9 shows every `buf generate` already contacts the BSR through three `remote:` plugins, with an
  empty local plugin cache, and that the googleapis module is already resolved and cached. The
  marginal cost is one round-trip; the vendored alternative would put a wire contract outside the
  codegen-drift gate (§ 5.2).
- *`MISSING_SCOPE` demonstrates nothing.* Accepted as a critique of the example, rejected as a
  critique of the table — § 9.4 now carries two examples where the override changes the answer.

**Deferred to the reader.** The three-PR split (§ 14.1) and the ADR-0018 amendment's timing (§ 16 Q1)
are decisions for the issue owner, not for this spec.

## 16. Open questions for review

1. **ADR-0018's amendment (§ 4.1)** — precondition of merging, or follow-up? The ADRs live in Notion.
2. **The three-PR split (§ 14.1)** — adopt, and re-cut the Linear issues?
3. ~~**`ResourceExhausted`/429 → `degraded`**~~ — **RESOLVED at the spec gate.** 429 gets its own
   `rate-limited` presentation, separated from `degraded` (§ 9.2). MEASURED (M13) while resolving
   it: nothing in this repo emits 429 today, so every 429 the SDK can see is OpenAI's quota
   arriving through the passthrough. A future Paigasus quota gets its distinct copy from § 9.4's
   override table, which is what that table is for.
4. ~~**Correlation id on a successful chat call**~~ — **RESOLVED at the spec gate.** Both
   `ChatCompletionResult` variants now carry `correlationId` and `requestId` (§ 8.2). The gateway's
   `CorrelationLayer` already stamps both on every response, so this cost nothing to supply.

## 17. The second adversarial challenge — SMA-625 (PR C)

§ 15 records the challenge that produced the three-PR split. This section records a **second**
challenge, run against §§ 8/9/10 alone once PRs A and B had merged. Its job was to find where those
sections had gone stale against shipped code. Verdict: **NEEDS REWORK**. Five BLOCKERs, all
reproduced against the source before folding.

**Folded in — BLOCKERs.**

1. **The passthrough and the idle timeout were mutually exclusive.** § 8 forbade reading the stream
   and § 10 asserted the *identical* object came back, while § 8.1 demanded an idle timeout — which
   needs an observer in the byte path. **Resolved: the idle timeout is dropped** (§ 8.3), stated as
   a non-goal with its reasoning, including the `TransformStream` backpressure defect that would
   have aborted healthy long completions. The pre-header deadline survives and § 8.4 now names its
   mechanism.
2. **Arm 3 was wrong about what a chat error body contains.** The gateway forwards a non-2xx
   upstream body **verbatim** (`chat.rs:19-20`, `:113-119`, `:138-142`), so arm 3 usually receives
   OpenAI's vocabulary, not the registry — and the content-type is *forced* to `application/json`,
   so the body may not parse at all. The registry assertion the spec cited covers `GatewayError`
   only. § 8.1 now tabulates all four branches and § 9.5 arm 3 splits into two sub-cases with
   explicit non-JSON and missing-`error` tolerance.
3. **The terminal-frame drift check could not run on the PR that breaks it.** No `rs/**` path is
   among the SDK test task's inputs, so editing `chat.rs:63` selected no TypeScript task — the
   identical defect § 11.1 fixed for AC 3. § 8.6 adds the input and its affected-graph control, as
   obligations 10 and 11, **without** a new `repo:*` gate.
4. **The file layout failed a test PR B had already merged.** `tests/server-guard.test.ts:19` pins
   `"import './server-guard.js';"` as a literal, which `src/errors/map-error.ts` cannot satisfy.
   Fixed by making the test compute each entry's expected specifier, which is what § 6.2 layer 3
   always claimed it did (obligation 9).
5. **The chat client had no type contract.** No request type, no filled-in response type, no error
   disposition. § 8.2 writes all three, and settles that a non-2xx **throws**.

**Folded in — MAJORs.** § 9.4's override table was inert — both its examples resolved identically
without it — so `Unimplemented` now maps to `generic` in § 9.2 and `CAPABILITY_DISABLED` lifts it,
making the mechanism change a real output; `PRINCIPAL_INACTIVE` joins it as a second load-bearing
entry, while `INVALID_REQUEST_SCHEMA` is relabelled honestly as a pin. § 9.4's `Record` did not
compose with the codec PR A shipped (`fromWireReason` returns a type including the sentinel;
`fromWireReason(asWireReason(r))` does not typecheck) — both fixed, via `presentationFor()` and a
guarded round-trip. Arm 3 gained the response headers; arm 4 gained the ids, which the terminal
frame cannot carry, changing the parser's signature. The parser contract gained six numbered points
covering `Uint8Array` input and a streaming `TextDecoder`, a buffer cap, single-return semantics,
and the three SSE delimiters. § 9.1 now states that `domain` is structurally `null` on three of four
arms, and makes that the primary argument for a reason-keyed table. § 10 marks its inherited tiers
and scopes the cancellation row to what a stub `fetch` can prove.

**Rejected, with evidence.**

- *"AC 4's four codes are never enumerated."* They are — in the Linear issue's own AC 3
  (`UNAUTHENTICATED`, `PERMISSION_DENIED`, `NOT_FOUND`, `UNAVAILABLE`). The critique searched the
  spec only. Named in § 10's AC 4 row so the next reader does not repeat the search.
- *"Arm 2 may not belong in PR C."* § 9.5 already answers this: IAM's REST surface is the external
  customer-facing API, so the helper is public API regardless of whether this package calls it.

**Decided by the issue owner at the spec gate**, not by this spec: dropping the idle timeout;
remapping `Unimplemented` to make the override table load-bearing; `PRINCIPAL_INACTIVE -> disabled`
while its two neighbours keep `forbidden`; and fixing the guard test rather than flattening the
file layout.

**Resolved at the spec gate, after the challenge.** Both of the challenge's two surviving
questions were answered rather than carried:

- **429 is no longer `degraded`.** It takes a new `rate-limited` presentation (§ 9.2), the ninth
  value in the union. Resolving it produced measurement **M13**: no `GatewayError` renders 429, no
  `ErrorClass` maps to `ResourceExhausted`, and `error.proto` declares no quota reason — so every
  429 reaching the SDK today is OpenAI's, through the passthrough. That single meaning is what lets
  the copy be specific. A Paigasus quota, when it exists, will need a registry reason, and § 9.4's
  override table gives it its own screen without a tenth presentation value.
- **A successful non-streaming chat call now exposes its ids.** `correlationId` and `requestId`
  move onto a shared `ChatResultBase`, so both variants carry them (§ 8.2). The headers were
  already on every response; only the type was withholding them.

**Still open:** nothing from this round.
