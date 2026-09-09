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
    chat.ts             the OpenAI-compatible chat client + parseTerminalFrame (§ 8)
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

**Nothing pins the ordering itself.** `CONTRACTS_GENERATE_INPUTS` (`ci_targets.py:393-400`) pins
`contracts:generate`'s *inputs*; `check_contracts_generate_inputs` (`ci_targets.py:1773-1786`) reads
`inputGlobs` and `inputFiles` and nothing else. No gate reads the task's `script:`. So the ordering,
the `set -euo pipefail`, and the `--path` narrowing are comments, and a dropped `--path` would
silently generate all of googleapis. Adding a script pin means a new registry obligation and is
**out of scope** (§ 14); this is recorded as a stated limitation, in the repo's own habit of naming
a residual rather than implying it is closed.

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

## 8. The chat client

`POST /v1/chat/completions` on the gateway, the one hand-written surface (§ 4.1).

- **Non-streaming:** `fetch`, JSON in, JSON out. A non-2xx body is the OpenAI envelope and goes to
  `mapError` (§ 9.4 arm 3).
- **Streaming:** the upstream `Response.body` is returned as a `ReadableStream` **passthrough**. The
  SDK does not read, buffer, decode or re-encode it. The gateway forwards upstream SSE chunks
  unbuffered (`chat.rs:128-137`); buffering here would undo that.

Two facts the passthrough forces into the API, both from the gateway's own code:

- A `stream: true` request that fails **before** the head is committed answers as plain JSON, not SSE
  (`chat.rs:139-141`). The caller must branch on `content-type`, not on its own `stream` flag, so the
  client returns `{ kind: 'json', … } | { kind: 'stream', body }` rather than making the caller guess.
- A failure **after** the head is committed cannot change the status, so the gateway injects exactly
  one terminal SSE frame carrying `code: "upstream-error"` and ends the stream (`chat.rs:63`).

**Mid-stream errors are not surfaced by this client, and the arm that maps them is a caller tool.**
The SDK never scans the stream, because scanning means buffering. So `parseTerminalFrame(chunk:
string): PaigasusError | null` is **exported** — a caller consuming its own stream calls it — rather
than being an internal arm no code path reaches. Its test is driven by a fixture holding the exact
frame from `chat.rs:63`, with a drift check asserting the fixture still matches that Rust constant
(the `repo:parity-corpus-drift` precedent). Without the fixture the test would hand-build the object
it expects and could not fail when the gateway's frame changes — which is the one drift it exists to
absorb.

## 9. The error model

### 9.1 One shape

```ts
interface PaigasusError {
  presentation: Presentation;
  domain: ErrorDomain | null;
  reason: ErrorReason | null;              // null === unmapped
  rawReason: string | null;                // what the wire actually said
  message: string;                         // human-readable. NEVER branched on.
  correlationId: string | null;
  requestId: string | null;
  retryable: boolean | null;               // null === the wire's "unknown"
  metadata: Readonly<Record<string, string>>;
  transport: { kind: 'grpc'; code: Code } | { kind: 'http'; status: number };
}

type Presentation =
  | 'relogin' | 'forbidden' | 'not-found' | 'degraded'
  | 'invalid-input' | 'conflict' | 'disabled' | 'generic';
```

`message` is carried for display and logging and is never an input to a branch. AC 2 is satisfied
structurally: no function in `src/errors/` reads `message`.

`retryable` is tri-state deliberately. The wire's values are `"true" | "false" | "unknown"`, and
collapsing `unknown` to `false` would assert a non-retryability the service declined to assert.
ADR-0019 decision 7 exists so clients stop inferring.

`transport` carries the raw code for logging. It is a number or a `Code`, never a `ConnectError` and
never `Headers`, so AC 4's "raw gRPC statuses never reach the browser" holds when the whole object is
serialized to a client component.

### 9.2 Two total mappings from transport status

Revision 1 gave gRPC codes only, while three of § 9.4's four arms are HTTP. Both tables are written,
and both are total by falling through to `generic`:

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

429 and 504 map to `degraded` rather than getting their own states: a gateway proxying OpenAI
produces both routinely, and both mean "try later", which is what `degraded` renders. `retryable`
carries the finer signal for a caller that wants it. This is a stated decision, not an omission.

`ResourceExhausted`/429 sitting under `degraded` is the one row a reviewer may want to revisit; it is
called out here rather than buried.

### 9.3 The wire-reason codec lives in `@paigasus/proto`

The TypeScript twin of `ErrorReason::{as_wire_reason, from_wire_reason}` goes in
`ts/packages/paigasus-proto/src/error.ts`, next to `capabilityWireKey`. That is where the precedent
lives, it is a contract concern rather than a transport one, and it stays available to a future
consumer that is not the SDK.

Both directions are **derived from the descriptor**: `ErrorReasonSchema.values` yields
`DescEnumValue[]` carrying `.name` — the raw proto name. `capabilityWireKey`'s doc comment records
why `.name` and not `.localName`: protobuf-es's shared-prefix heuristic (`findEnumSharedPrefix`)
degrades for the whole enum if any value's short name is empty or starts with a digit.

`fromWireReason` reproduces the Rust `is_wire_token` **allow-list** exactly —
`^[a-z][a-z0-9]*(-[a-z0-9]+)*$` — checked *before* any case transform.

**MEASURED (M7), on Node 24.16.0:** `"ınternal".toUpperCase() === "INTERNAL"` is `true`, and
`"ſlug-conflict"` folds to `SLUG_CONFLICT`. JavaScript folds U+0131 and U+017F exactly as
`str::to_uppercase` does, so a deny-list check would let both reconstruct valid proto names. The
allow-list is load-bearing, not decoration. A parity test asserts the TS parser rejects the same ten
inputs the Rust test rejects (`error.rs:287-302`).

### 9.4 The presentation override table — AC 3

`src/errors/presentation.ts` holds

```ts
type Entry = Presentation | 'from-transport';
const PRESENTATION: Record<Exclude<ErrorReason, ErrorReason.UNSPECIFIED>, Entry> = { … };
```

**MEASURED (M8), on TypeScript 6.0.3:** omitting a member yields
`TS2741: Property '[ErrorReason.INTERNAL]' is missing`, naming it. Revision 1 declared
`Record<ErrorReason, Presentation>`, which was unwritable twice over — it demanded an entry for the
`UNSPECIFIED` sentinel the test skips, making the table 58 keys rather than 57, and `'from-transport'`
is not a `Presentation`. The corrected type is 57 keys.

Two mechanisms, both required:

1. **The total `Record`** makes a missing reason a compile error.
2. **A table test** iterates `ErrorReasonSchema.values`, skips the sentinel, and asserts each reason
   round-trips through `fromWireReason(asWireReason(r))` and has an entry. This is AC 3 verbatim.

The type check alone is not enough — a refactor to `Partial<Record<…>>` would silently switch it off,
and the test notices. The test alone is not enough — it runs later. Both are kept.

**What the table is for, with two real examples.** Revision 1 justified it with `MISSING_SCOPE`,
which was a bad example: it is a 500, 500 is already `generic`, and the entry changed nothing. The
genuine cases:

- **`INVALID_REQUEST_SCHEMA` (906) — the cross-service divergence.** IAM answers **422**, the gateway
  answers **400**, for the identical wire code. `error.proto:239-247` says so in the registry itself
  and warns that *"a consumer mapping code -> status must not assume it is one-to-one"*. Under § 9.2
  both already land on `invalid-input`, so the table's job here is to **pin** that agreement: an
  explicit entry means a future change to either status cannot silently split the presentation of one
  code across two services.
- **`CAPABILITY_DISABLED` (904)** is gRPC `Code::Unimplemented` with no HTTP form at all
  (`convert.rs:96-104`). `Unimplemented` reads as "this build cannot do that", but the product meaning
  is "this deployment turned that capability off" — a different screen. `disabled` exists in § 9.1
  for it, and the entry is what selects it.

Every other reason takes the literal `'from-transport'`, so all 57 are a reviewed decision rather
than a default.

**The table is keyed on `reason`, not `(domain, reason)`.** Q1's sweep found exactly one reason whose
transport status differs by site, `INVALID_REQUEST_SCHEMA`, and both sites resolve to the same
presentation. Keying on the pair would double the table to normalize nothing. If a second divergence
appears where the presentations genuinely differ, the key must become the pair — recorded here so the
decision is revisited rather than inherited.

### 9.5 Four inputs, one output

`mapError` accepts:

1. **A `ConnectError`** — `err.findDetails(ErrorInfoSchema)[0]` gives `(reason, domain, metadata)`.
   `metadata` carries `retryable`, and `correlation_id`/`request_id` **when the error was raised
   inside a request scope** (`convert.rs:59-74`) — omitted, not nulled, outside one. `err.code` gives
   the gRPC `Code`; `err.metadata` is *"a union of response headers and trailers"*
   (`connect-error.d.ts:22-24`), the fallback for the correlation id.
2. **An IAM HTTP response** — `{ status, headers, body }` where body is `{error:{code,message}}`. The
   body carries **no** correlation id and **no** retryable; both are headers
   (`paigasus-correlation-id`, `paigasus-request-id`, `paigasus-retryable`,
   `paigasus-observability/src/correlation.rs:20,31-33`). AC 2 asks for "a generic fallback plus
   correlation id", so the headers are not optional input.
3. **A gateway HTTP response** — same envelope plus `{message,type,param,code}`. `code` is drawn from
   the same registry (asserted `gateway/adapters/http/error.rs:296-309`). `type` is **not** read: it
   takes two values and both are coarser than `code`. `param` enters `metadata` when present.
4. **A parsed terminal SSE frame**, via the exported `parseTerminalFrame` (§ 8). `transport` is
   `{ kind: 'http', status: 200 }` because the head was already committed; `retryable` is `null`,
   since that frame deliberately carries no retryable signal (`chat.rs:56-62`).

**Arm 2 has no in-SDK caller, and that is deliberate.** § 4.1 moves every IAM HTTP surface to gRPC,
so nothing in this package constructs arm 2's argument. It is kept as a **public helper** because
IAM's REST surface still exists and is the external customer-facing API (ADR-0018 consequence 6); an
app calling it directly should map its errors the same way. Its test drives it directly. Stated so
the arm is not mistaken for a code path the SDK exercises.

**`metadata` is the ErrorInfo map minus the three lifted keys.** `retryable`, `correlation_id` and
`request_id` are removed after being read into their typed fields, so a caller cannot branch on a raw
duplicate that disagrees with the parsed one. `capability`, `field` and `param` survive.

**Unknown reason handling (AC 2).** `rawReason` always holds what the wire said. When `fromWireReason`
rejects it, `reason` is `null`, `presentation` falls back to the transport-derived value, and
`correlationId` is preserved. An unrecognized code degrades to a generic presentation plus a
user-reportable id — never a throw, never an empty message.

**The two system-retirement 409s are not special-cased.** They add sibling keys next to the standard
`error` object rather than replacing it (`system_retirement.rs:111-126`). `mapError` reads
`error.code` and ignores the siblings; a caller wanting the surviving-grants list reads the body.

## 10. Testing

| Tier | What it proves |
|---|---|
| Wire-reason codec parity | Both directions over all 57 reasons; the ten malformed inputs the Rust test rejects |
| Presentation totality (AC 3) | Every descriptor value has an entry; `INVALID_REQUEST_SCHEMA` and `CAPABILITY_DISABLED` resolve as § 9.4 states |
| Transport-status tables (§ 9.2) | Both tables, including the `generic` fall-through |
| `mapError` — gRPC | A `ConnectError` carrying a real `ErrorInfo` detail round-trips; `correlation_id`/`retryable` read from `metadata`; the three lifted keys are absent from `metadata` |
| `mapError` — HTTP | Both envelopes; correlation id from **headers**; tri-state retryable |
| `mapError` — degradation (AC 2) | Unknown reason yields `reason: null`, keeps `rawReason` and `correlationId` |
| AC 4 mapping | Each of the four codes yields its state |
| Message-independence (AC 2) | One wire error with three different `message` strings maps to three identical objects modulo `message` |
| Transport cache | Equal options return the same object; differing options do not; two `Auth` values on one cached transport produce two `authorization` headers; `disposeTransports()` lets vitest exit |
| Server-only structure (AC 1) | Every guarded `exports` entry imports the guard as its first import statement, driven off `package.json` |
| Chat | Non-streaming maps a non-2xx; streaming returns the identical `ReadableStream` object; `parseTerminalFrame` against the pinned fixture |
| Terminal-frame drift | The fixture still matches `chat.rs:63` |

**Two coverage gaps, stated rather than implied.**

*No live-service tier.* The SDK has no server to talk to in CI. Transport behaviour is tested through
interceptors and a stub `fetch`. This suite proves the SDK *forms* correct requests and *interprets*
correct responses; it does not prove IAM accepts them. Standing a service up is SMA-509/SMA-510's
integration surface.

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
3. **`ci/affected-graph/ci_targets.py:393-400`** — `CONTRACTS_GENERATE_INPUTS` is a strict-equality
   pin. Adding `contracts/buf.gen.googleapis.yaml` to `contracts:generate` reds
   `repo:affected-smoke` until this tuple is updated.

### 11.3 The packages

4. **`@paigasus/proto`'s public surface** — § 3 promised it and Revision 1 never specified it. The
   root barrel (`src/index.ts`) gains, through the existing single `"."` export:
   `ErrorReason`, `ErrorReasonSchema`, `ErrorDomain`, `ErrorDomainSchema`, `ErrorInfoSchema`
   (re-exported from the generated googleapis module), `asWireReason`, `fromWireReason`,
   `asWireDomain`, `fromWireDomain`, and the seven `iam/v1` service descriptors —
   `TenancyService`, `AuthnService`, `AuthorizationService`, `ServiceAccountService`, `AuditService`,
   `UserService`, `OutboxService` — with their request/response types. No `./generated/*` subpath is
   added: that would make the generated layout public API, so a codegen reshuffle would become a
   breaking change for consumers.
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
| M5 | Reversing the two `buf generate` calls leaves `error_details_pb.ts` deleted, and the **drift step** reds | pending |
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
3. **`ResourceExhausted`/429 → `degraded`** (§ 9.2) — the one presentation row worth a second opinion.
