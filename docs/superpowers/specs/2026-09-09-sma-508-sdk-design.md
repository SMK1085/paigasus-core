<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-508 — `@paigasus/sdk`: Connect-ES gRPC clients and error mapping

**Issue:** [SMA-508](https://linear.app/smaschek/issue/SMA-508), split into
[SMA-624](https://linear.app/smaschek/issue/SMA-624) (PR A, merged `9f57d6e4`),
[SMA-508](https://linear.app/smaschek/issue/SMA-508) (PR B, merged `2262801d`) and
[SMA-625](https://linear.app/smaschek/issue/SMA-625) (PR C, §§ 8-10)
**ADR:** ADR-0018 (Connect-ES over gRPC; no OpenAPI surface), ADR-0019 (Canonical error model, incl. Amendment A1)
**Design source:** Frontend Architecture Scoping §§ 6, 7
**Date:** 2026-09-09
**Revision:** 3 — §§ 8-10 re-challenged on their own after PRs A and B merged, and rewritten.
§ 15.1 records what the second challenge changed. §§ 1-7 and 11-14 are Revision 2 plus the edits
those findings forced. § 15 records the first challenge.

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
  src/error.ts                       asWireReason / fromWireReason — the codec (§ 9.3)
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
    chat.ts             the "./chat" ENTRY; the chat client + createTerminalFrameParser (§ 8)
    errors.ts           the "./errors" ENTRY; server-guarded; re-exports mapError + the types
    errors/
      types.ts          the "./errors/types" ENTRY; PaigasusError, Presentation — NO guard (§ 6.3)
      presentation.ts   the reason -> presentation override table (§ 9.4) — internal
      transport-status.ts  gRPC Code and HTTP status -> Presentation (§ 9.2) — internal
      map-error.ts      mapError() — internal, reached through ./errors
  tests/
```

Tests live in `tests/`, matching `@paigasus/ui` and `@paigasus/next-config`. The two files added to
`@paigasus/proto` follow *that* package's colocated `*.test.ts` convention instead — a package keeps
its own habit.

**Revision 2 moved the `./errors` entry to `src/errors.ts`, and this is a correctness fix, not a
preference.** Revision 1 put every `errors` file one directory deep and named no entry file at all.
PR B's merged `tests/server-guard.test.ts` pins the guard import as a **literal string** —
`GUARD_IMPORT = "import './server-guard.js';"` at `:19`, compared with `toBe` at `:52` — for every
guarded entry in the `exports` map. A guarded entry at `src/errors/…` needs
`import '../server-guard.js';` and fails that comparison. § 6.2 layer 3 said the test resolves "the
path relative to each entry"; PR B implemented a string compare instead. So PR C would red a merged
test on its first commit.

Every **guarded entry** therefore sits at `src/` root, beside the existing `src/iam.ts` and
`src/index.ts`. The internals stay under `src/errors/`, where no guard is needed because the entry
evaluates it. `src/errors/types.ts` stays one deep: it is unguarded, and the unguarded assertion
(`:57-60`) is `not.toContain`, which holds at any depth. `UNGUARDED_ENTRIES` in that test already
lists `'./errors/types'`, so PR B anticipated this entry.

**The root barrel names each re-export explicitly. It does not use `export *` for the new modules.**
If both `chat.ts` and `errors.ts` re-exported `PaigasusError`, the ES semantics TypeScript follows
would exclude the ambiguous name **silently** rather than erroring — the exact failure
`ts/packages/paigasus-proto/src/iam.ts:5-14` documents for `ServiceInfo`, and the reason PR A split
that package's surface. `src/index.ts` also re-exports the types from `errors/types.ts`, so
`mapError`'s return type can be named from the root entry.

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
  "./errors":       "./src/errors.ts",
  "./errors/types": "./src/errors/types.ts"
}
```

Subpaths exist so a caller that only maps an error does not pull `@connectrpc/connect-node` and its
HTTP/2 stack into its module graph.

**Corrected in the fix wave before PR (SMA-625, item 8):** this table said `./src/errors/map-error.ts`.
The branch ships `./src/errors.ts` as the guarded `./errors` entry, one directory shallower than the
internal `map-error.ts` module it re-exports from. `tests/server-guard.test.ts` pins the guard import
as the literal string `"import './server-guard.js';"` and compares it with `toBe`, so a guarded entry
one directory deeper — where the relative path would read `'../server-guard.js'` — cannot pass that
test. `src/errors.ts` is therefore the only depth this entry can live at.

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

**Revision 2 rewrote this section.** Revision 1 described response handling only. It gave no client
signature, no credential, and no `fetch` seam. It also stated a timeout contract that contradicted
its own passthrough rule. § 8.1 to § 8.5 replace it.

### 8.1 The client surface

The endpoint is authenticated. `chat.rs:5-6` records that the G5 `require_iam_auth` middleware runs
before the handler. `GatewayError::MissingBearer` answers `401` with code `missing-authorization`
(`error.rs:122-128`). So the chat client needs a credential, and § 7.4's rule applies here too: a
forgotten token must be a compile error, not a `401`.

```ts
interface ChatClientOptions {
  baseUrl: string;
  /** Bound for the response HEAD only. Default 10_000 ms. See § 8.4. */
  headerTimeoutMs?: number;
  /** The injection seam. Defaults to `globalThis.fetch`, read per call. */
  fetch?: typeof globalThis.fetch;
}

export function createChatClient(opts: ChatClientOptions, auth: { bearer: string }): ChatClient;
```

Three decisions, each stated because an implementer would otherwise guess:

- **`auth` is a required parameter and its type is not the `Auth` union.** § 7.4's union carries an
  `{ anonymous: true }` arm for the IAM health check. No path through `chat_completions` accepts an
  anonymous caller, so an anonymous arm here would be a legal value with no legal use. The narrower
  type makes that a compile error. This is deliberately NOT a decision about SMA-627, which asks a
  different question about the gRPC transport.
- **The chat client does not use `getTransport` and shares none of its cache.** It calls `fetch`; it
  builds no `Transport`. § 7.1's cache key rule does not reach it, and nothing here is cached.
- **`fetch` is an optional field, read per call.** A module-load capture would make `vi.stubGlobal`
  useless, and § 10's deadline, cancellation and streaming rows all need a stub. Reading
  `globalThis.fetch` per call keeps the default correct when a host installs a dispatcher later.

### 8.2 Two response shapes, and who maps an error

The client returns a discriminated result. It never throws for a mapped error.

```ts
type ChatResult =
  | { kind: 'json'; status: number; body: unknown; correlationId: string | null; requestId: string | null }
  | { kind: 'stream'; body: ReadableStream<Uint8Array>; correlationId: string | null; requestId: string | null }
  | { kind: 'error'; error: PaigasusError };
```

**Widened in the fix wave before PR (SMA-625, item 4), a deliberate deviation from the two-field arms
above.** `correlation.rs:174-175` sets both id headers on every response head with no status guard, so
a `200 text/event-stream` head carries them too — but the arm as first written discarded them. When
such a stream then fails mid-flight, the failure routes through § 8.4's parser and `mapHttp(200, new
Headers(), body)`, which has no head to read and so reports `correlationId: null` — the one case this
package built a parser for was the one case with no reportable id. Reading the two ids off the
original response head, on both success arms, closes that gap. This adds no `Headers` object to
`ChatResult`, so AC 3 (no raw transport type reaches the browser) is unaffected.

The caller must branch on `content-type`, not on its own `stream` flag. A `stream: true` request that
fails **before** the head is committed answers as plain JSON, not SSE (`chat.rs:139-141`). Revision 1
got this right and it stands.

**Revision 1 left one question unanswered: does the SDK call `mapError`, or does the caller?** The
answer is the SDK, on the non-2xx path, and the third arm above carries the result. A caller that
receives `{ kind: 'error' }` has an already-mapped `PaigasusError` and needs no error knowledge of its
own. `mapError` stays exported for arm 2 (§ 9.5), which has no in-SDK caller.

### 8.3 Streaming is a passthrough

The upstream `Response.body` is returned as the identical `ReadableStream` object. The SDK does not
read, buffer, decode or re-encode it. The gateway forwards upstream chunks unbuffered
(`chat.rs:194-220`, the comment and code at `:210-211`), and buffering here would undo that.

**Citation corrected.** Revision 1 cited `chat.rs:128-137` for the unbuffered forwarding. Those lines
build the success-stream response. The forwarding itself is at `chat.rs:194-220`.

A failure **after** the head is committed cannot change the status. The gateway therefore injects one
terminal SSE frame carrying `code: "upstream-error"` and ends the stream.

### 8.4 The terminal-frame parser

The SDK never scans the stream, because scanning means buffering. A terminal-frame parser is
therefore **exported**, and a caller consuming its own stream drives it.

A `ReadableStream` chunk is not guaranteed to hold one complete SSE record. An SSE record ends at a
blank line, and a chunk boundary can fall anywhere. A parser over one bare chunk is lossy exactly on
the split-frame case — the terminal error split across two chunks — which is the case the parser
exists to catch. The exported shape is a stateful incremental parser.

```ts
export function createTerminalFrameParser(committedStatus: number, ids?: FrameIds): {
  /** Every terminal error frame COMPLETED by this chunk, in order. Empty when none completes. */
  push(chunk: Uint8Array): PaigasusError[];
};
```

Four corrections to Revision 1, each from the challenge:

1. **The return type is an array, not `PaigasusError | null`.** Revision 1's doc comment said the
   parser "yields each complete SSE record it can form" while its signature could carry one value.
   § 10 also tests "two frames in one chunk", which the single-value signature cannot express.
2. **`push` takes `Uint8Array`, not `string`.** Revision 1 argued at length that a chunk boundary
   can fall anywhere, then handed the caller a `string` parser without saying who decodes. A
   per-chunk `TextDecoder().decode()` splits a multi-byte character on exactly the same boundary.
   The parser owns a `TextDecoder` and calls `decode(chunk, { stream: true })`, so it holds the
   trailing partial character as well as the trailing partial record.
3. **The record delimiter is `\n\n`.** That is what the gateway emits (`chat.rs:63`). A `\r\n\r\n`
   delimiter is legal SSE and is not produced here; the parser accepts both, because accepting it
   costs one alternation and rejecting it would be a silent miss.
4. **The parser holds only the trailing partial record and the trailing partial character.** It does
   not reintroduce the buffering the passthrough avoids.

**The drift check needs a mechanism, and Revision 1 gave it none.** Revision 1 asked for "a drift
check asserting the fixture still matches that Rust constant (the `repo:parity-corpus-drift`
precedent)". That precedent is a `repo:*` Moon gate with its own `inputs`. § 11.3 says this PR adds no
`repo:*` gate. As an in-package vitest test the check is vacuous: `paigasus-sdk-ts:test`'s inputs are
`@group(sources)`, `@group(tests)`, `package.json`, `/ts/pnpm-lock.yaml`, plus the two globs PR B
appended. `chat.rs` is in none of them. Editing `TERMINAL_SSE_ERROR` selects no SDK task, and Moon
serves a cached PASS. This is the same vacuity § 11.1 measured for AC 3 (M11) and fixed, left unfixed
here.

**The fix — obligation 9 (§ 11.2).** `paigasus-sdk-ts:test` gains
`/rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs` as an `input`, and the test reads the
constant out of that Rust source **by name**, not by line number. A new
`run_task_case_ci "gateway->sdk"` case is the control on that input, exactly as obligation 2 is the
control on § 11.1's.

Two properties of this choice, stated rather than left to be discovered:

- **Keying on the constant NAME is load-bearing.** Any edit above line 63 moves the constant. A
  line-number fixture would then compare the wrong line and could pass while the frame changed.
- **The cost is that every `chat.rs` edit re-runs the SDK suite.** The suite is vitest with no live
  service, so this is seconds. The alternative — a committed shared fixture plus a Rust `#[test]`
  beside `the_terminal_sse_frame_carries_a_registered_code` (`chat.rs:253-262`) — moves the assertion
  to the gateway side and avoids the re-runs, at the cost of a cross-workspace fixture file. **This
  spec chooses the input, and records the alternative so a reviewer can overrule it.**

### 8.5 Deadlines and cancellation

§ 7.2 gives the gRPC transport a 10 s default deadline. Revision 1's § 8.1 gave the chat client four
rules. Two survive, one is corrected, and one is **removed**.

**Kept — a pre-header timeout, at the same 10 s default.** The two surfaces agree on the wait for
response headers.

**Kept — the client accepts an optional `AbortSignal` and forwards it to `fetch`.**

**Corrected — the pre-header timeout must NOT be `AbortSignal.timeout`.** The obvious build is
`AbortSignal.any([callerSignal, AbortSignal.timeout(10_000)])`. That signal keeps running after the
headers arrive and aborts the streaming body at 10 s, which is the behaviour this section forbids.
Every chat completion longer than ten seconds — the normal case for the product — would be truncated
in production, and § 10's stalled-header row would stay green, because it passes under both builds.
The correct shape is a manually driven `AbortController` whose timer is cleared the moment the `fetch`
promise settles:

```ts
const ctl = new AbortController();
const timer = setTimeout(() => ctl.abort(new DOMException('header timeout', 'TimeoutError')), headerTimeoutMs);
try {
  const res = await fetchImpl(url, { ..., signal: anySignal(ctl.signal, callerSignal) });
  return res;
} finally {
  clearTimeout(timer);      // the body stream outlives this scope, deliberately
}
```

§ 10 gains a row that distinguishes the two builds: a stub `fetch` resolves its headers immediately,
then emits a chunk **after** the pre-header window has passed. The correct build delivers the chunk.
`headerTimeoutMs` is a per-call option so this test does not wait ten real seconds.
`AbortSignal.timeout` is not driven by vitest fake timers — confirm this in the test rather than
assuming it.

**REMOVED — the idle timeout.** Revision 1 said "the stream fails if no bytes arrive for the idle
window". Detecting idleness means observing the byte flow, and there are only three ways to do it.
Wrapping `response.body` in a `TransformStream` reads the stream and returns a **different** object,
which breaks § 8.3's rule and § 10's identity assertion at once. Pushing the job to the caller means
it is not the SDK's contract. Reaching undici's `bodyTimeout` needs `setGlobalDispatcher`, a
process-global side effect a library must not take, or a direct `undici` dependency that § 11.3 does
not list. An implementer reading Revision 1 would pick the `TransformStream` route, because it is the
only one the spec makes visible, and would silently defeat the unbuffered forwarding the gateway
went to trouble to preserve.

So the SDK states the narrower contract it can actually keep: **once the head is committed, a stalled
stream is the caller's responsibility.** The caller's own `AbortSignal` is the lever, and it is
already forwarded. This is a reduction in scope against Revision 1 and is called out for review
rather than buried.

**Cancellation propagates because the stream is the platform's own object.** If the caller cancels the
returned `ReadableStream`, undici cancels the upstream connection. The SDK adds nothing here, and
§ 10's row is corrected accordingly (see § 10).

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
  transport:
    | { kind: 'grpc'; code: Code; codeName: string }
    | { kind: 'http'; status: number }
    | { kind: 'transport'; cause: 'timeout' | 'network' | 'aborted' };
}

type Presentation =
  | 'relogin' | 'forbidden' | 'not-found' | 'degraded'
  | 'invalid-input' | 'conflict' | 'disabled' | 'generic';
```

`message` is carried for display and logging and is never an input to a branch. SMA-625's AC 1 is
satisfied structurally: no function in `src/errors/` reads `message`.

**`PaigasusError` is a plain data object. It is never an `Error` subclass, and `mapError` returns it
and never throws it.** Revision 1 declared an `interface`, which is right, but said nothing about the
runtime shape and had no test for it. The name reads like a throwable, and `ConnectError` — the thing
arm 1 consumes — is an `Error` subclass, so `class PaigasusError extends Error` is the natural thing
to write. React's server-to-client serializer rejects class instances, so that choice would invalidate
§ 6.3 and AC 3 at once, and only when SMA-510 wires a console months later. § 10 gains a row
asserting `Object.getPrototypeOf(result) === Object.prototype`.

`rawDomain` exists for the same reason as `rawReason`: if a newer service sends an unregistered
domain, `domain` is `null` and the wire value would otherwise be lost. ADR-0019 decision 9 makes the
whole `(domain, reason)` pair the basis of consumer branching, so keeping half the pair loggable and
discarding the other half would be worse than keeping neither.

`retryable` is tri-state deliberately. The wire's values are `"true" | "false" | "unknown"`, and
collapsing `unknown` to `false` would assert a non-retryability the service declined to assert.
ADR-0019 decision 7 exists so clients stop inferring.

**`transport` gained a third arm.** Revision 1's union was gRPC or HTTP, and could not represent "no
response at all". § 8.5's pre-header timeout, a caller abort, a DNS failure and a refused connection
all make `fetch` reject with a `TypeError` or `DOMException`, which is none of Revision 1's four
input arms. The gRPC side has a home for this — arm 1b handles a `ConnectError` with no detail, and
connect maps an abort to `Code.Canceled` — and the chat side had none. § 8.5's stated goal is that
the two surfaces agree, and on the failure path they did not.

**`transport.code` carries `codeName` beside it.** `Code` is a runtime enum object from
`@connectrpc/connect`. Typing the field as `Code` is a type-only use and erases under
`verbatimModuleSyntax`, so a client component can name it. Reading the number back as a name would
need a value import of a server-side dependency, so the name is carried instead. `transport` is for
logging; a client branches on `presentation`. That is what AC 3's "raw gRPC statuses never reach the
browser" requires — `PaigasusError` holds no `ConnectError` and no `Headers`.

### 9.2 Two total mappings from transport status

Both tables are total. Both fall through to `generic`.

| gRPC `Code` | Presentation | | HTTP status | Presentation |
|---|---|---|---|---|
| `Unauthenticated` | `relogin` | | 401 | `relogin` |
| `PermissionDenied` | `forbidden` | | 403 | `forbidden` |
| `NotFound` | `not-found` | | 404 | `not-found` |
| `Unavailable` | `degraded` | | 502, 503, 504 | `degraded` |
| `DeadlineExceeded` | `degraded` | | 408 | `degraded` |
| `InvalidArgument` | `invalid-input` | | 400, 413, 415, 422 | `invalid-input` |
| `AlreadyExists`, `FailedPrecondition`, `Aborted` | `conflict` | | 409 | `conflict` |
| `Unimplemented` | `disabled` | | 501 | `disabled` |
| `ResourceExhausted` | `degraded` | | 429 | `degraded` |
| everything else | `generic` | | everything else | `generic` |

The third `transport` arm maps by `cause`: `timeout` and `network` yield `degraded`, `aborted` yields
`generic`. A caller abort is not a service fault and must not render as one.

429 and 504 map to `degraded` rather than getting their own states: a gateway proxying OpenAI produces
both routinely, and both mean "try later", which is what `degraded` renders. `retryable` carries the
finer signal for a caller that wants it. **The issue owner reviewed this row and kept it** (§ 16 Q3 is
now closed), so it is a decision, not an omission.

**The four codes SMA-625's AC 3 names, in one place.** Revision 1's § 10 had a row called "AC 4
mapping — each of the four codes yields its state" and the four codes appeared nowhere in the spec.
They are:

| Code | Presentation | Rendered as |
|---|---|---|
| `Code.Unauthenticated` / 401 | `relogin` | re-login |
| `Code.PermissionDenied` / 403 | `forbidden` | the 403 view |
| `Code.NotFound` / 404 | `not-found` | the 404 view |
| `Code.Unavailable` / 502, 503, 504 | `degraded` | degraded |

**Totality over `Code` has a concrete trap. MEASURED (M13), on `@connectrpc/connect` 2.2.0 under Node
24.18.1:** `Code` is a non-const numeric enum, so `Object.values(Code)` yields **32** entries — 16
numeric (1 to 16) and 16 string names. A totality test written the obvious way iterates the strings
too and asserts nothing useful about them. The test must enumerate
`Object.values(Code).filter((v) => typeof v === 'number')` and assert each resolves to a member of the
`Presentation` union. Totality is the assertion that carries that § 10 row; the spot checks are
secondary, because a transcription of the table into the test proves only that someone transcribed it.

### 9.3 The wire-reason codec lives in `@paigasus/proto`

**This landed in PR A (SMA-624).** `ts/packages/paigasus-proto/src/error.ts` holds `asWireReason`,
`fromWireReason`, `asWireDomain` and `fromWireDomain`, and the root barrel exports all four. The
allow-list `^[a-z][a-z0-9]*(-[a-z0-9]+)*$` and the ten-input rejection parity test are at
`src/error.test.ts`. Nothing in SMA-625 changes this section; it is kept for context.

### 9.4 The presentation override table

`src/errors/presentation.ts` holds

```ts
type Entry = Presentation | 'from-transport';
const PRESENTATION: Record<Exclude<ErrorReason, ErrorReason.UNSPECIFIED>, Entry> = { … };
```

**MEASURED (M8), on TypeScript 6.0.3:** omitting a member yields
`TS2741: Property '[ErrorReason.INTERNAL]' is missing`, naming it. The table is 57 keys. Re-confirmed
by count: the proto and the generated TS both declare 58 `ErrorReason` members including the sentinel.

Two mechanisms, both required:

1. **The total `Record`** makes a missing reason a compile error.
2. **A table test** iterates `ErrorReasonSchema.values`, skips the sentinel, and asserts each reason
   round-trips through `fromWireReason(asWireReason(r))` and has an entry. This is SMA-625's AC 2.

The type check alone is not enough — a refactor to `Partial<Record<…>>` would silently switch it off,
and the test notices. The test alone is not enough — it runs later. Both are kept.

**What the table is for. Revision 2 adds a third example, and it is the strongest of the three.**

- **`UPSTREAM_ERROR` (307) — the override that fixes a visibly wrong default.** Arm 4 sets
  `transport: { kind: 'http', status: <the committed status> }`, because the head was already
  committed. § 9.2's HTTP
  table has no 200 row, so it falls through to `generic`. Under Revision 1's "every other reason takes
  `'from-transport'`" rule, the mid-stream failure the whole parser exists to catch would render as a
  generic error. `chat.rs:59-62` documents the opposite intent: `upstream-error` "is by construction
  the transient mid-stream case … a client that recognizes this code already knows it may retry".
  `degraded` is the state § 9.2 says renders "try later". The entry is `'degraded'`.
  `UPSTREAM_UNAVAILABLE` (305) and `UPSTREAM_TIMEOUT` (306) take an explicit `'degraded'` entry too.
  Their transport statuses already reach `degraded`, so the entry **pins** an agreement rather than
  changing an answer — the same job the `INVALID_REQUEST_SCHEMA` entry does.
- **`INVALID_REQUEST_SCHEMA` (906) — the cross-service divergence.** IAM answers **422**, the gateway
  answers **400**, for the identical wire code. `error.proto:239-247` says so in the registry itself
  and warns that "a consumer mapping code -> status must not assume it is one-to-one". Under § 9.2
  both already land on `invalid-input`, so the entry **pins** that agreement: a future change to
  either status cannot silently split one code's presentation across two services.
- **`CAPABILITY_DISABLED` (904)** is gRPC `Code::Unimplemented` with no HTTP form at all
  (`convert.rs:96-104`, and no HTTP call site exists in IAM). `Unimplemented` reads as "this build
  cannot do that", but the product meaning is "this deployment turned that capability off" — a
  different screen. `disabled` exists in § 9.1 for it, and the entry selects it.

Every other reason takes the literal `'from-transport'`, so all 57 are a reviewed decision rather than
a default.

**The table is keyed on `reason`, not `(domain, reason)`. Re-checked against the three upstream
codes.** Q1's sweep found exactly one reason whose transport status differs by site,
`INVALID_REQUEST_SCHEMA`, and both sites resolve to the same presentation. The three upstream codes
are gateway-only, so they add no divergence and the conclusion stands. If a second divergence appears
where the presentations genuinely differ, the key must become the pair — recorded here so the decision
is revisited rather than inherited.

### 9.5 Five inputs, one output

Revision 1 had four arms. Arm 5 is new (§ 9.1).

1. **A `ConnectError`.** `err.findDetails(ErrorInfoSchema)[0]` can be `undefined` — a `ConnectError`
   raised by a network failure, a proxy, or a connection reset carries a gRPC status and no
   `ErrorInfo` detail. This is a reachable path, so arm 1 branches before reading the detail:
   - **Detail present:** the detail gives `(reason, domain, metadata)`. `metadata` carries
     `retryable`, and `correlation_id`/`request_id` **when the error was raised inside a request
     scope** (`convert.rs:59-74`) — omitted, not nulled, outside one.
   - **Detail absent:** `domain`, `reason`, `rawReason` and `rawDomain` are `null`. `retryable` is
     `null` — the wire asserted nothing. `metadata` is empty and `requestId` is `null`.
     `presentation` comes from the gRPC status table, including its `generic` fallback. Without this
     branch the SDK throws while mapping an error, which turns a recoverable upstream failure into an
     unhandled exception in the BFF.
   - **`message` is `err.rawMessage`, never `err.message`, on both branches of this arm.** Corrected
     in the fix wave before PR (SMA-625, item 8): `ConnectError.message` prefixes the status code
     (e.g. `"[not_found] first"`), so reading it would both break AC 1's message-independence claim
     and leak a raw gRPC status into a user-facing string. `err.rawMessage` carries the message with
     no such prefix.
   - **The two correlation-id spellings are different and both are read, in this order.** The
     `ErrorInfo` metadata key is `correlation_id` (`convert.rs:70`). The response header is
     `paigasus-correlation-id` (`correlation.rs:31`). `err.metadata` is "a union of response headers
     and trailers", so it carries the header spelling. The metadata key wins when both are present.
     Revision 1 used both spellings without distinguishing them.
2. **An IAM HTTP response** — `{ status, headers, body }` where body is `{error:{code,message}}`
   (`adapters/http/error.rs:35`, key set asserted at `:106-110`). The body carries **no** correlation
   id and **no** retryable; both are headers (`paigasus-correlation-id`, `paigasus-request-id`,
   `paigasus-retryable`, `correlation.rs:20,31,33`). `metadata` is `{}`: IAM's HTTP envelope carries
   no metadata map. In particular `field` is **gRPC-only** — `status_to_grpc` puts the failing field
   name into `ErrorInfo.metadata` (`convert.rs:129-130`), and the HTTP body has no room for it.
3. **A gateway HTTP response** — the OpenAI envelope
   `{ error: { message, type, param, code } }` (`adapters/http/error.rs:27-45`, pinned at `:334-342`).
   **Revision 1's premise here was false for a whole class of real responses.** It said `code` "is
   drawn from the same registry". That assertion (`error.rs:296-309`) covers only errors the gateway
   itself generates. `chat.rs:113-119` forwards a non-2xx upstream response **verbatim, including
   OpenAI's own error envelope**, and `chat.rs:139-141` does the same for a failed `stream: true`
   request. So:
   - The body may be gateway-generated **or** an upstream passthrough.
   - `code` and `param` are **nullable**. OpenAI's `code` is its own vocabulary
     (`insufficient_quota`, `context_length_exceeded`) or JSON `null`.
   - A `null` `code` yields `rawReason: null` and `reason: null`. A non-null `code` always populates
     `rawReason`; `fromWireReason` rejects an underscore token, which is the correct degradation to
     `reason: null`.
   - `param` enters `metadata` only when it is a non-null string. `type` is **not** read: it takes two
     values and both are coarser than `code`.
   - The same three headers as arm 2 are read. The gateway sets `paigasus-retryable` on every error it
     renders (`error.rs:204`) and the correlation layer defaults it for responses no renderer owns
     (`correlation.rs:68-77`). An upstream passthrough may carry none, and then `retryable` is `null`.
   - The rate-limit `429` § 9.2 discusses arrives through this arm, as an upstream passthrough.
4. **A parsed terminal SSE frame**, via § 8.4's parser. `transport` carries the status the gateway
   actually COMMITTED — usually 200, but a stream committed on another 2xx must report that, and
   the arm also accepts the head's correlation and request ids
   because the head was already committed. `retryable` is `null`: that frame deliberately carries no
   retryable signal (`chat.rs:56-62`). `metadata` is `{}`. `presentation` is `degraded`, via § 9.4's
   `UPSTREAM_ERROR` entry.
5. **A transport failure with no response at all** — a rejected `fetch`. `transport` is
   `{ kind: 'transport', cause }`. `domain`, `reason`, `rawReason` and `rawDomain` are `null`,
   `retryable` is `null`, `metadata` is `{}`, and `message` is the rejection's message. The `cause` is
   `timeout` for § 8.5's pre-header abort, `aborted` for a caller abort, and `network` otherwise.

**`mapError` is total over a malformed body, and Revision 1 did not say so.** Its totality claim was
scoped to an unrecognized *code*. It said nothing about a body that is not JSON at all (an ingress
HTML `502` in front of the gateway), an empty body, or a JSON body with no `error` key. `mapError`
runs in the BFF on the failure path, where a throw converts a recoverable upstream failure into an
unhandled exception — the same failure mode arm 1b exists to prevent. So arms 2 and 3 accept an
already-parsed body **or** a parse failure, and a missing or malformed `error` object yields
`reason: null`, `rawReason: null`, `message` from the status text, and the transport-derived
presentation.

**Arm 2 has no in-SDK caller, and that is deliberate.** § 4.1 moves every IAM HTTP surface to gRPC, so
nothing in this package constructs arm 2's argument. It is kept as a **public helper** because IAM's
REST surface still exists and is the external customer-facing API (ADR-0018 consequence 6); an app
calling it directly should map its errors the same way. Its test drives it directly.

**`metadata` is the ErrorInfo map minus the three lifted keys** — on arm 1 only. `retryable`,
`correlation_id` and `request_id` are removed after being read into their typed fields, so a caller
cannot branch on a raw duplicate that disagrees with the parsed one. `capability` and `field` survive.
Arms 2, 4 and 5 yield `{}`; arm 3 yields `{ param }` when `param` is a non-null string.

**Unknown reason and domain handling.** `rawReason` always holds what the wire said. When
`fromWireReason` rejects it, `reason` is `null`, `presentation` falls back to the transport-derived
value, and `correlationId` is preserved. An unrecognized code degrades to a generic presentation plus
a user-reportable id — never a throw, never an empty message. `rawDomain` is populated from
`ErrorInfo.domain` verbatim, whether or not `fromWireDomain` resolves it.

**The two system-retirement 409s are not special-cased.** They add sibling keys next to the standard
`error` object rather than replacing it (`system_retirement.rs:143-155` is the insertion site;
`:111-126` builds the payloads). `mapError` reads `error.code` and ignores the siblings; a caller
wanting the surviving-grants list reads the body.

### 9.6 A consumer must be able to name a reason

**This is new in Revision 2, and it is the gap that would have made the package unusable for its
stated purpose.**

§ 1 says the package exists because "ADR-0019 decision 9 requires consumers to branch on
`(domain, reason)`; there is no consumer to do so". § 9.1 types those fields as `ErrorReason` and
`ErrorDomain`. But the eslint boundary already merged in this repo bans apps from importing
`@paigasus/proto` at all — `paigasus/boundaries/apps` in `ts/packages/paigasus-next-config/src/eslint.mjs`
covers `@paigasus/proto` and `@paigasus/proto/**`, and that file records at `:28-31` that type imports
are banned alongside value imports, deliberately. `@paigasus/sdk` re-exports the seven IAM services
but not the registry. So an app would receive `reason: 906` and have no legal way to write
`ErrorReason.INVALID_REQUEST_SCHEMA`.

**`src/errors/types.ts` therefore re-exports `ErrorReason` and `ErrorDomain` as values**, not only as
types, from `@paigasus/proto`. This puts two small enum objects in the client bundle deliberately.
They are plain frozen objects with no `server-only` import, so the entry stays unguarded and
`tests/server-guard.test.ts`'s unguarded assertion still holds.

**A note for SMA-510's author, not a change here.** The `paigasus/boundaries/app-shell` block bans
`@paigasus/sdk` and `@paigasus/sdk/**` outright, which includes the client-safe `./errors/types`
entry § 6.3 exists to provide. That block is inert until `packages/paigasus-app-shell` exists. It will
need a `!@paigasus/sdk/errors/types` negation pair, in the **doubled** form `eslint.mjs:24-26` says is
load-bearing. Flagged rather than fixed, because the package it governs does not exist yet.

## 10. Testing

**Revision 2 rebuilt this table.** Revision 1 listed fourteen rows. Three of them had already shipped
in PRs A and B, two named a mechanism that does not exist, and two were tautologies. The `Landed`
column says where each row already holds, so PR C's real scope is visible.

| Tier | What it proves | Landed |
|---|---|---|
| Wire-reason codec parity | Both directions over all 57 reasons; the ten malformed inputs the Rust test rejects | **PR A** — `paigasus-proto/src/error.test.ts:47,60` |
| Transport cache | Equal options return the same object; differing options do not; two clients with different `Auth` produce two different `authorization` headers and neither carries the other's token; `disposeTransports()` lets vitest exit | **PR B** — `tests/transport.test.ts:18-38`, `tests/iam.test.ts:81-123` |
| Server-only structure | Every guarded `exports` entry imports the guard as its first import statement, driven off `package.json` | **PR B** — `tests/server-guard.test.ts` |
| Presentation totality (AC 2) | Every descriptor value has an entry; `UPSTREAM_ERROR`, `INVALID_REQUEST_SCHEMA` and `CAPABILITY_DISABLED` resolve as § 9.4 states | new |
| Transport-status totality (§ 9.2) | `Object.values(Code).filter(v => typeof v === 'number')` — all 16 — resolve to a `Presentation`; the HTTP table over its listed statuses; one unknown status per table hits `generic`; the third arm's three causes | new |
| The four AC-3 codes | Each of § 9.2's four named codes yields its stated presentation, on both the gRPC and the HTTP side | new |
| `mapError` — gRPC, detail present | A `ConnectError` carrying a real `ErrorInfo` detail round-trips; `correlation_id`/`retryable` read from `metadata`; the three lifted keys are absent from `metadata`; `capability` and `field` survive | new |
| `mapError` — gRPC, detail absent | A `ConnectError` with no `ErrorInfo` detail falls back to the gRPC status table with a message-only object and throws nothing | new |
| `mapError` — HTTP, both envelopes | IAM's `{error:{code,message}}` and the gateway's `{error:{message,type,param,code}}`; correlation id from **headers**; tri-state retryable | new |
| `mapError` — an upstream passthrough (§ 9.5 arm 3) | A real OpenAI `429` envelope with `code: "insufficient_quota"` and `param: null` yields `reason: null`, keeps `rawReason`, and presents `degraded` from the status | new |
| `mapError` — a malformed body | A non-JSON HTML `502`, an empty body, and a JSON body with no `error` key each yield a transport-derived presentation and throw nothing | new |
| `mapError` — a transport failure (§ 9.5 arm 5) | Each of the three causes maps to its presentation; `aborted` is `generic`, not `degraded` | new |
| `mapError` — degradation (AC 1) | Unknown reason yields `reason: null`, keeps `rawReason` and `correlationId`; unknown domain yields `domain: null`, keeps `rawDomain` | new |
| Message-independence (AC 1) | One wire error with three different `message` strings maps to three identical objects modulo `message` | new |
| `PaigasusError` is a plain object | `Object.getPrototypeOf(result) === Object.prototype`; it is not an `Error` instance | new |
| Chat — auth | The outgoing request carries `authorization: Bearer <token>`, mirroring PR B's gRPC row | new |
| Chat — non-streaming | A non-2xx JSON response yields `{ kind: 'error' }` carrying a mapped `PaigasusError` | new |
| Chat — streaming passthrough (AC 4) | The returned stream is **identity-equal** to the `Response.body` the stubbed `fetch` returned | new |
| Chat — cancellation | Cancelling the returned stream invokes the stub source's `cancel`. End-to-end propagation to a socket is **not** tested — see the gaps below | new |
| Chat — the pre-header deadline fires | A stub `fetch` that never resolves its headers fails at `headerTimeoutMs` | new |
| Chat — the deadline does NOT outlive the head | A stub resolves headers immediately, then emits a chunk **after** `headerTimeoutMs` has passed. The chunk must arrive. This is the row that separates the `AbortController` build from the `AbortSignal.timeout` build (§ 8.5) | new |
| Terminal-frame parser | Against the frame read from `chat.rs`: one whole frame; a frame split across two chunks; two frames in one chunk (both returned, in order); a partial trailing record that never completes; a multi-byte character split across a chunk boundary | new |
| Terminal-frame drift | The parser's expected frame is read from `chat.rs` **by constant name**, and `chat.rs` is an input of `paigasus-sdk-ts:test` (§ 8.4, obligation 9) | new |
| Affected-graph control | A new `run_task_case_ci "gateway->sdk"` selects the SDK suite when `chat.rs` changes | new |

**Two rows Revision 1 carried that are deliberately gone.**

*The idle-timeout row.* § 8.5 removed the contract, so there is nothing to test.

*"Streaming returns the identical `ReadableStream` object" as a separate row from cancellation.* The
identity assertion is what makes platform cancellation possible, so the two are one row plus its
consequence. Revision 1's cancellation row — "an abandoned stream propagates cancellation to the
upstream connection" — was untestable as written: with a stub `fetch` there is no upstream connection,
and § 10 itself states there is no live-service tier. If § 8.3's passthrough is literal, propagation is
undici's behaviour and a test of it asserts the platform. The corrected row names the observable the
SDK actually controls.

**Three coverage gaps, stated rather than implied.**

*No live-service tier.* The SDK has no server to talk to in CI. Transport behaviour is tested through
interceptors and a stub `fetch`. This suite proves the SDK *forms* correct requests and *interprets*
correct responses; it does not prove IAM or the gateway accepts them. Standing a service up is
SMA-509/SMA-510's integration surface.

*AC 1 of SMA-508 is not proven by a build.* See § 6.2.

*Cancellation does not reach a socket in any test.* The SDK returns the platform's own stream, so the
propagation belongs to undici. Only an integration tier could prove it.

**One decision the `mapError` gRPC row depends on.** A hand-constructed
`new ConnectError(msg, code, headers, [{ desc: ErrorInfoSchema, value: {…} }])` takes `findDetails`'s
**outgoing** branch — a plain `create()` — not the wire branch that runs `fromBinary`. The outgoing
shape proves nothing about decoding a `grpc-status-details-bin` trailer of the kind
`tonic_types::ErrorDetails::with_error_info` produces (`convert.rs:79`). **The fixture uses the
incoming wire shape**, `{ type: 'google.rpc.ErrorInfo', value: toBinary(ErrorInfoSchema, …) }`, so the
decode path is the one under test.
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

**PR B shipped this block, so SMA-625 changes one line of it.** `test` gains a second cross-workspace
input (§ 8.4, obligation 9):

```yaml
  test:
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
      - 'vitest.config.ts'
      - '/rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs'   # NEW, § 8.4
```

Without that line the terminal-frame drift check is vacuous in exactly the way M11 measured for AC 3:
an edit to `TERMINAL_SSE_ERROR` selects no SDK task, and Moon serves a cached PASS on the one PR the
check exists to catch.

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
9. **A new `run_task_case_ci "gateway->sdk"`** — added by Revision 2, and the one registration
   obligation SMA-625 still owns. It is anchored on
   `rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs` and asserts the edit selects
   `paigasus-sdk-ts:test`. This is the only control on § 11.1's new input line, exactly as
   obligation 2 is the only control on the `@paigasus/proto` one. Without it, a future edit dropping
   that input leaves the terminal-frame drift check vacuous and nothing reds.

   Re-measure the expected set rather than transcribing it. A `chat.rs` edit also selects the
   gateway crate's own Rust tasks, so this case's expected set is **not** a copy of `proto->sdk`'s.

   Obligations 1 to 8 all landed in PRs A and B. Obligation 9 is new work.

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

**The three-PR split re-cut the acceptance criteria, and Revision 1's numbers were stale.** This
document was written as one spec for one issue. §§ 9 and 10 then referred to "AC 2", "AC 3" and
"AC 4" by SMA-508's ORIGINAL numbering. SMA-508 has since been re-cut and renumbered — its AC 4 is
now "a new `proto->sdk` case", an affected-graph obligation — so a reader following those numbers
lands on the wrong criterion. Both tables below are keyed to the CURRENT Linear issues.

### 12.1 SMA-625 — the criteria this PR must meet

**One wording correction against the Linear issue.** SMA-625's AC 1 says an unknown reason yields a
"generic fallback". The implemented behaviour, which § 9.5 describes and § 10 tests, is the
**transport-derived** presentation — so an unknown reason on a `404` presents as `not-found`, not as
`generic`. `generic` is only what the transport tables themselves fall through to. The distinction
matters: degrading a `404` to `generic` would lose information the status already carried.

| AC | Text | Where |
|---|---|---|
| 1 | Branch on `(domain, reason)` only, never message text; an unknown reason degrades to the TRANSPORT-derived presentation plus the correlation id | § 9.1, § 9.5; the message-independence and degradation rows in § 10 |
| 2 | A table test driven off the registry | § 9.4 — the total `Record` (M8) **plus** a descriptor-driven test, made reachable by § 11.1 (M11) |
| 3 | The four codes → four states; raw statuses never reach the browser | § 9.2, including the four-code table added in Revision 2; `PaigasusError` holds no `ConnectError` and no `Headers` (§ 6.3, § 9.1) |
| 4 | Chat streaming is a `ReadableStream` passthrough | § 8.3; § 10's identity-equality row |

### 12.2 SMA-508 — met by PR B, listed so the numbering is unambiguous

| AC | Text | Where |
|---|---|---|
| 1 | `import 'server-only'`; a client import fails the build | § 6.2 — four layers, **and a stated gap**: enforced structurally, not proven by a build |
| 2 | `contracts->proto` updated in the same change | § 4.2, § 11.2 obligation 1 — one id, measured (M4) |
| 3 | The SDK's task `inputs` name `@paigasus/proto` | § 11.1 (M11, M11b) |
| 4 | A new `proto->sdk` affected-graph case | § 11.2 obligation 2 |
| 5 | A forgotten token is a compile error | § 7.4 |
| 6 | An explicit default deadline | § 7.2 |

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
| M13 | `Object.values(Code)` yields **32** entries — 16 numeric (1-16) and 16 string names — on `@connectrpc/connect` 2.2.0 / Node 24.18.1, so a naive totality test iterates strings too | **TAKEN** |
| M14 | PR B's `tests/server-guard.test.ts` compares the guard import as a LITERAL string (`:19`, `:52`), so a guarded entry one directory deep fails it | **TAKEN** |
| M15 | The gateway's chat endpoint requires a bearer (`chat.rs:5-6`); `MissingBearer` answers 401 `missing-authorization` (`error.rs:122-128`) | **TAKEN** |
| M16 | Apps may not import `@paigasus/proto` — `paigasus/boundaries/apps` bans it, and type imports are banned alongside value imports (`eslint.mjs:28-31`) | **TAKEN** |
| M5 | Reversing the two `buf generate` calls leaves `error_details_pb.ts` deleted, and the **drift step** reds | **TAKEN** |
| M6 | Equal transport options return one object; differing options return two; two `Auth` values give two headers | pending |
| M10 | A client component `import type`-ing from the guarded `./errors` entry builds | pending |
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
| **C** | § 8 chat, § 9 error model and tables, § 10's new rows, obligation 9 | Needs A's `ErrorInfoSchema` and B's package. Holds all four of SMA-625's ACs (§ 12.1). |

SMA-508's AC 2 and AC 4 land in **B**, which is where the dependency edge is created.

**ADOPTED, and executed.** PR A is SMA-624, merged as `9f57d6e4`. PR B is SMA-508, merged as
`2262801d`. PR C is SMA-625. Revision 3 re-challenged §§ 8-10 on their own against the merged
result of A and B; § 15.1 records what that found.

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

### 15.1 What the SECOND challenge changed (Revision 2, SMA-625)

§§ 8-10 were re-challenged on their own after PRs A and B merged, because the split left them
standing against a codebase that already held the other two PRs' output. Six BLOCKERs and eleven
MAJORs were filed. Every one was verified against the merged code before it was folded in.

**Folded in — the six BLOCKERs.**

1. **§ 8 had no configuration or authorization surface.** It described response handling only, and
   never said how a caller supplies a base URL or a credential — on the one surface that carries the
   customer's API key. § 8.1 is new (M15).
2. **§ 8.1's idle timeout contradicted § 8's own passthrough rule** and named no mechanism that could
   implement it. Removed, with the narrower contract stated in its place (§ 8.5).
3. **The pre-header timeout could not be built as `AbortSignal.timeout`** without truncating every
   chat completion longer than ten seconds, and Revision 1's test row passed under both the correct
   and the broken build. § 8.5 names the `AbortController` shape and § 10 gains the row that
   separates them.
4. **The terminal-frame drift check had no mechanism.** `chat.rs` was not an input of any SDK task,
   so the check was vacuous in exactly the way M11 measured for AC 3. § 8.4 and obligation 9 fix it.
5. **`src/errors/map-error.ts` would have failed a test PR B already merged** (M14). § 3.1 moves
   every guarded entry to `src/` root.
6. **The AC numbers were stale against the re-cut issues.** § 12 is now two tables, and § 9.2 names
   the four codes that appeared nowhere in Revision 1.

**Folded in — the MAJORs.** § 9.5 arm 3's false premise, that the gateway's `code` is always
registry-drawn, when `chat.rs:113-119` forwards OpenAI's own envelope verbatim with a nullable `code`
and `param`; a fifth `mapError` arm and a third `transport` arm for a rejected `fetch`; totality over
a malformed or non-JSON body; `UPSTREAM_ERROR`'s `degraded` override, which is the strongest of the
three override examples because the default is visibly wrong; § 9.6, without which no app could name a
reason at all (M16); `PaigasusError` being a plain object rather than an `Error` subclass; the `fetch`
injection seam; the cancellation row rewritten from a tautology; `Code` totality (M13); the three
§ 10 rows that had already landed; and the parser's signature, which contradicted its own doc comment.

**Folded in — the MINORs.** Arm 3's retryable headers; `metadata` on each HTTP arm; `field` being
gRPC-only; the root barrel's star-export ambiguity; the parser taking `Uint8Array` rather than
`string`; and `codeName` beside `transport.code`.

**Corrected rather than accepted as filed.** The challenge said the four AC codes "are never
enumerated anywhere in the spec". They were enumerated in SMA-625's Linear description and every one
had a row in § 9.2's table. The real defect was narrower: § 10's row named them without listing them,
under a stale AC number. Fixed as § 9.2's four-code table.

**Two citations corrected, from an independent fact-check of every reference in §§ 8-10.**
`chat.rs:128-137` (unbuffered forwarding) is really `chat.rs:194-220`, and
`system_retirement.rs:111-126` builds the payloads while `:143-155` inserts the sibling keys.
Everything else checked — the terminal frame at `chat.rs:63` byte-for-byte, the three header
constants, `error.proto:239-247`, the 906 and 904 values, the `is_wire_token` allow-list and its ten
rejected inputs, and the count of 57 reasons excluding the sentinel — was accurate.

### 15.2 What PR #231 contributed (a parallel implementation, merged in)

SMA-625 was implemented **twice**, concurrently and independently, by two sessions that did not see
each other. PR #231 and PR #232 were both complete and both green. The issue owner chose to keep
#232 as the base and port #231's better decisions onto it. Five landed:

1. **The SSE record delimiter is `/(?:\r\n|\r|\n){2}/`** — any two consecutive line terminators, with
   `\r\n` first in the alternation so a CRLF is consumed whole. The previous `/\r?\n\r?\n/` required
   an LF in each half, so a **bare CR** never matched and the terminal frame was silently never
   completed. MEASURED: reverting it reds exactly that one row.
2. **Multiple `data:` lines join with `\n`, not `''`.** The WHATWG grammar appends U+000A after each
   `data` field's value. Stated honestly: on the frames this gateway emits — always one `data:`
   line — the two are indistinguishable, so no test discriminates them. This is a correctness
   change to the receiver, not a bug fix. The counter-argument that `''` protects a JSON string
   split across two lines was **rejected**: that producer has emitted a broken document, and
   joining with `''` would corrupt every legitimate multi-line payload to hide it.
3. **The parser takes the COMMITTED STATUS** rather than hardcoding 200 (§ 9.5 arm 4).
4. **The parser takes the head's IDS**, so the one failure with no ids of its own still carries a
   reportable correlation id.
5. **`tests/server-guard.test.ts` computes each entry's expected guard specifier from that entry's
   own directory** instead of pinning one literal. This is what § 6.2 layer 3 always described, and
   it removes the constraint that every guarded entry live at `src/` root — the constraint § 3.1's
   layout was bent around. The unguarded assertion PARSES rather than pattern-matches, for the
   reason the rest of that file already records: a first attempt with a regex flagged the prose in
   `errors/types.ts` that says the file must not gain a guard.

Two further defects came from #231's own review rounds and applied here unchanged:

6. **`JSON.stringify(request)` sat inside the `try` wrapping `fetch`**, so a caller passing a
   circular reference or a bigint had their own bug reported back as a gateway `degraded`. It is
   now serialized before the try and before the deadline timer, so a stringify failure can neither
   be misattributed nor leak a timer.
7. **`??` does not fall through on an empty string**, so an `ErrorInfo` carrying
   `correlation_id: ""` suppressed the header fallback and produced a blank id. A `firstNonEmpty`
   helper replaces it.

One caveat inherited knowingly: the parser `.trim()`s each `data:` line where the grammar strips
exactly one leading space. Harmless for JSON payloads, which ignore surrounding whitespace, and
recorded here rather than inherited silently.

**Not re-litigated, by the issue owner's decision.** § 9.2's `ResourceExhausted`/429 row stays
`degraded`; SMA-627 stays out of scope; the three-PR split stands.

## 16. Open questions for review

All three of Revision 1's questions are now **closed**.

1. **ADR-0018's amendment (§ 4.1)** — precondition of merging, or follow-up? The ADRs live in Notion.
2. **The three-PR split (§ 14.1)** — adopt, and re-cut the Linear issues? **ADOPTED.** SMA-624 (A)
   and SMA-508 (B) have merged; SMA-625 is C.
3. **`ResourceExhausted`/429 → `degraded`** (§ 9.2) — the one presentation row worth a second
   opinion. **KEPT as `degraded`,** by the issue owner, on this revision.

### 16.1 What Revision 3 put to the reader — all three now CONFIRMED

The issue owner reviewed all three at the spec-approval gate and confirmed each as written. They are
recorded here as decisions, not as open questions.

1. **§ 8.5 removes the idle timeout** from the SDK's contract. That is a reduction in scope against
   Revision 1. The alternative is a `TransformStream` wrapper, which breaks the passthrough and
   SMA-625's AC 4. Confirm the reduction is the right trade.
2. **§ 8.4 chooses a Moon `input` over a shared fixture** for the terminal-frame drift check. The
   cost is that every `chat.rs` edit re-runs the SDK's vitest suite. The alternative — a committed
   fixture plus a Rust `#[test]` — is recorded in § 8.4 and can be chosen instead.
3. **§ 9.6 puts `ErrorReason` and `ErrorDomain` into the client-safe surface as VALUES.** Two small
   enum objects enter the client bundle. Without them no app can branch on a reason, which is the
   package's stated purpose, but it is still a deliberate bundle cost.
