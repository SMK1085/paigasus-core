<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-508 — `@paigasus/sdk`: Connect-ES gRPC clients and error mapping

**Issue:** [SMA-508](https://linear.app/smaschek/issue/SMA-508)
**ADR:** ADR-0018 (Connect-ES over gRPC; no OpenAPI surface), ADR-0019 (Canonical error model, incl. Amendment A1)
**Design source:** Frontend Architecture Scoping §§ 6, 7
**Date:** 2026-09-09
**Revision:** 1 — pre-challenge.

**Versions this spec is written against.** `@connectrpc/connect` 2.2.0, `@connectrpc/connect-node`
2.2.0, `@bufbuild/protobuf` 2.14.1, protoc-gen-es v2.13.0 (pinned in `buf.gen.yaml`), buf 1.70.0,
TypeScript 6.0.3, vitest 5.0.0, Moon 2.5.3, pnpm 11.3.0.

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

A real `@paigasus/sdk` (`ts/packages/paigasus-sdk`, already scaffolded: `package.json`, `moon.yml`,
`tsconfig.json`, `.releaserc.json`, a stub `src/index.ts`). Plus:

- a widened public surface on `@paigasus/proto`;
- a second, TypeScript-only `buf generate` invocation producing the `google.rpc.ErrorInfo`
  descriptor;
- three registration edits (§ 11).

Every new source file carries an SPDX header. `private: true` and source-only, matching
`@paigasus/ui` and `@paigasus/next-config`.

### 3.1 Layout

```
ts/packages/paigasus-sdk/
  package.json          exports ".", "./iam", "./chat", "./errors"
  moon.yml              dependsOn: ['paigasus-proto-ts']
  tsconfig.json
  vitest.config.ts      node env; ssr.resolve.conditions for server-only
  src/
    index.ts            `import 'server-only'`; re-exports the three entries
    server-guard.ts     the single `import 'server-only'` site (§ 6.2)
    transport.ts        per-base-URL transport cache + the auth interceptor
    auth.ts             the bearer ContextKey and its accessors
    iam.ts              typed client factories over the seven IAM services
    chat.ts             the OpenAI-compatible chat client (§ 8)
    errors/
      types.ts          PaigasusError and the presentation union
      presentation.ts   the reason -> presentation table (AC 3)
      from-connect.ts   ConnectError  -> PaigasusError
      from-http.ts      IAM + gateway envelopes -> PaigasusError
      map-error.ts      the public mapError() entry
  tests/
```

## 4. Three corrections this spec makes to its own inputs

These are stated up front because two of them change the acceptance criteria.

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
hand-written surface. This *shrinks* the drift risk ADR-0018 names as its own main downside, rather
than accepting it for three surfaces that no longer need it.

**This needs an ADR-0018 amendment**, recording that decision 5's list was overtaken by
implementation. The decision itself — "HTTP-only surfaces get a small hand-written fetch client" —
stands unchanged; only its membership shrinks. Filed as part of this issue's Linear comment, not as
a silent divergence.

### 4.2 AC 5 quotes a stale expected set, and "every downstream app" is empty

AC 5 quotes the `contracts->proto` expected set as six ids. It has **seven** — it gained
`paigasus-service-info-rs` (SMA-505/SMA-524) after the issue was written
(`ci/affected-graph/run.sh:258-259`).

More consequentially: **MEASURED**, `moon query projects` reports `paigasus-console-ts` depending on
`paigasus-next-config-ts` and `paigasus-ui-ts` only, and no project in `ts/` declares a `dependsOn`
on `paigasus-sdk-ts` or lists `@paigasus/sdk` in a `package.json`. The case uses
`--downstream deep` over the **project** graph (`run.sh:32-36`), so the delta is exactly one id:

```
+ paigasus-sdk-ts
```

AC 5's "and every downstream app" is vacuous today. This spec does **not** wire the console to the
SDK — that is SMA-509/SMA-510's work — so no app is added. § 13 M4 records the measurement that
proves it, and the case is re-measured rather than transcribed.

Also measured: TypeScript project edges are **not** auto-derived. `.moon/toolchains.yml` and
`.moon/workspace.yml` carry no node dependency-sync setting, and every TS `dependsOn` in the repo is
hand-authored. Adding `"@paigasus/proto": "workspace:*"` to the SDK's `package.json` alone creates
no project edge — `moon.yml` must gain `dependsOn: ['paigasus-proto-ts']` by hand. This is the
opposite of the Rust toolchain's behaviour for `path =` deps (CLAUDE.md).

### 4.3 The registry is 57 reasons, not 46

SMA-507's handoff comment on this issue says "the registry currently holds 46 reasons". It holds
**57** plus the `UNSPECIFIED` sentinel (`error.proto:59-262`; the Rust mirror test asserts 57 at
`rs/crates/libs/paigasus-proto/src/error.rs:230`). The number matters only because AC 3's table must
cover all of them; deriving the table's key set from the descriptor rather than hand-listing it
makes the count self-correcting.

## 5. `google.rpc.ErrorInfo` in TypeScript — ADR-0019 A1.4 resolved

A1.4 left this open: *"the TypeScript path is unverified — if Connect-ES requires a generated
`ErrorInfo` descriptor, that module has to be generated after all."*

**It does.** `ConnectError.findDetails` is
`findDetails<Desc extends DescMessage>(desc: Desc): MessageShape<Desc>[]`
(`@connectrpc/connect@2.2.0`, `connect-error.d.ts:84-85`). There is no schema-free path: the second
overload takes a `Registry`, which is also built from descriptors.

**MEASURED** (§ 13 M1): `contracts/buf.yaml` already declares `buf.build/googleapis/googleapis` as a
dep with a `buf.lock` entry, and

```
buf generate --template buf.gen.googleapis.yaml \
  buf.build/googleapis/googleapis --path google/rpc/error_details.proto
```

exits 0 and emits exactly **one** file, `google/rpc/error_details_pb.ts` (662 lines). It is
self-contained: its only non-generated import is `Duration` from `@bufbuild/protobuf/wkt`, which is
runtime, not a second generated module. It exports `ErrorInfoSchema` alongside thirteen other
`google.rpc` detail schemas, all tree-shakeable.

A1.4's fear was that referencing `ErrorInfo` from `error.proto` would emit Rust and Python pointing
at modules the run never produced. That fear was correct and this approach does not trigger it:
`error.proto` still imports nothing, and the second template runs **only** the TypeScript plugin
against a different input module. Rust and Python are untouched.

### 5.1 The ordering constraint is load-bearing

`contracts/buf.gen.yaml` sets `clean: true`, which wipes each `out:` directory before regeneration.
The googleapis template writes into the *same* tree
(`ts/packages/paigasus-proto/src/generated/`). So:

- the main template must run **first** (wipes the tree, writes `paigasus/**`);
- the googleapis template must run **second**, with `clean: false` (writes `google/rpc/**`).

Reversed, the second run's output is destroyed by the first. `contracts:generate` therefore becomes
a `script:` block with an explicit `set -euo pipefail` — Moon does not enable errexit for `script:`
blocks and takes the block's status from its last command, so without it a failed first `buf
generate` followed by a successful second one exits 0 (the same latent defect
`paigasus-console-ts:build` documents).

The ordering is asserted, not just commented: a test in `@paigasus/proto` imports `ErrorInfoSchema`
and asserts `typeName === 'google.rpc.ErrorInfo'`. If the ordering regresses, the file is absent and
the test fails at import — before the codegen-drift gate would even run.

## 6. Package shape

### 6.1 Four entry points

```json
"exports": {
  ".":        "./src/index.ts",
  "./iam":    "./src/iam.ts",
  "./chat":   "./src/chat.ts",
  "./errors": "./src/errors/map-error.ts"
}
```

Subpaths exist so a caller that only renders an error does not pull `@connectrpc/connect-node` and
its HTTP/2 stack into its module graph. The root re-exports all three, matching `@paigasus/ui`'s
single-barrel habit for the common case.

### 6.2 `server-only` — AC 1

`src/server-guard.ts` holds the single `import 'server-only';` and nothing else. Every other entry
imports it first. One site, so a future entry point that forgets the guard is a visible omission
rather than a copied line that drifted.

Four layers hold AC 1, mirroring SMA-506 § 4.3:

1. **`import 'server-only'`.** Its exports map is `{ "react-server": "./empty.js", "default":
   "./index.js" }` and `index.js` is one unconditional `throw`. A client component importing the SDK
   fails the build.
2. **The eslint boundary rule**, already live and already naming this package
   (`ts/packages/paigasus-next-config/src/eslint.mjs:91-104`): the SDK may import `@paigasus/proto`
   and nothing else in the `@paigasus/*` namespace, and `react`/`react-dom`/`next` are banned
   outright. This is why the SDK cannot import `@paigasus/auth` (§ 9) and cannot reach
   `next/headers`.
3. **A structural test** asserting every file named in the `exports` map imports `./server-guard.js`
   on its first line, driven off `package.json` rather than a hand-listed array — so a fifth entry
   point is covered the day it is added.
4. **No React, no JSX, no DOM types** in `tsconfig.json`'s `lib`/`types`.

`server-only` guards the **client bundle only**. Next sets the `react-server` condition for the
middleware layer too, so it resolves to `empty.js` there and is a no-op — the trap SMA-502 measured
(M6). The SDK is not middleware-reachable by design, but this is noted so a future edge-runtime
caller is not surprised. No `NEXT_RUNTIME` check is added here: unlike
`@paigasus/next-config/runtime`, the SDK reads no environment.

### 6.3 vitest

`environment: 'node'`, `include: ['tests/**/*.test.ts']`, and **both** `resolve.conditions` and
`ssr.resolve.conditions` set to `['react-server', 'node', 'import', 'default']`. Vitest 5 resolves a
node-environment test's imports through `ssr.resolve.conditions`, not the top-level key — measured
on SMA-502 and recorded in CLAUDE.md. Listing `react-server` alone would drop `import`/`default` and
break source-exports `.ts` resolution for `@paigasus/proto`.

## 7. Transport, caching and per-call auth

ADR-0018 decision 6: *"Transports are cached per base URL; authorization is attached per call via an
interceptor reading the request-scoped session. No token ever lives in a cached object."*

The interceptor lives on the transport, and the transport is cached — so the interceptor cannot
close over a token. Connect-ES v2's `ContextValues` is exactly the mechanism (**MEASURED**, § 13 M2):
`CallOptions.contextValues?: ContextValues` (`call-options.d.ts:30-34`) and the interceptor's request
carries `readonly contextValues: ContextValues` (`interceptor.d.ts:143`).

```ts
// auth.ts
export const bearerToken = createContextKey<string | null>(null);

// transport.ts
const transports = new Map<string, Transport>();

export function getTransport(baseUrl: string): Transport {
  let t = transports.get(baseUrl);
  if (t === undefined) {
    t = createGrpcTransport({ baseUrl, interceptors: [attachBearer] });
    transports.set(baseUrl, t);
  }
  return t;
}

const attachBearer: Interceptor = (next) => (req) => {
  const token = req.contextValues.get(bearerToken);
  if (token !== null) req.header.set('authorization', `Bearer ${token}`);
  return next(req);
};
```

The cache is a module-level `Map` keyed by base URL. It holds transports only — the token is never
an argument to `getTransport`, which is what makes the "no token in a cached object" rule structural
rather than a convention. A test asserts two calls with the same base URL return the same object and
two different tokens on that shared transport produce two different `authorization` headers.

`createGrpcTransport` from `@connectrpc/connect-node` speaks gRPC over HTTP/2, matching the
gateway's existing h2c channel to IAM. ADR-0018 consequence 4 already records that a deployment
terminating HTTP/1.1 in front of IAM breaks this.

### 7.1 Why the token is not read from a session here

The SDK's boundary rule bans `@paigasus/auth` and `next`. It therefore cannot call `getSession()`
and cannot reach `next/headers`. The *caller* — a server component or route handler in an app —
reads the session and passes the token into `contextValues`. That keeps the SDK a pure transport
layer with no ambient state, and it is what makes it testable without a Redis fixture.

## 8. The chat client

`POST /v1/chat/completions` on the gateway, the one hand-written surface (§ 4.1).

- **Non-streaming:** `fetch`, JSON in, JSON out. On a non-2xx, the body is the OpenAI envelope and
  is handed to `mapError` (§ 9.4).
- **Streaming:** the upstream `Response.body` is returned as a `ReadableStream` **passthrough** — the
  SDK does not read, buffer, decode or re-encode it. The gateway itself forwards upstream SSE chunks
  unbuffered (`chat.rs:128-137`), and buffering here would undo that.

Two facts the passthrough forces into the error model, both from the gateway's own code:

- A `stream: true` request that fails **before** the head is committed answers as plain JSON, not
  SSE (`chat.rs:139-141`). So the caller must branch on `content-type`, not on the request's
  `stream` flag. The client returns a discriminated result — `{ kind: 'json', … }` or
  `{ kind: 'stream', body }` — rather than making the caller guess.
- A failure **after** the head is committed cannot change the HTTP status, so the gateway injects
  exactly one terminal SSE frame carrying `code: "upstream-error"` and ends the stream
  (`chat.rs:63`). That frame is *inside* the stream the SDK passes through untouched. `mapError`
  accepts a parsed terminal frame (§ 9.4) so a caller that chooses to inspect its own stream can map
  it, but the SDK does not scan the stream to find it. Scanning would mean buffering.

That last point is a deliberate limitation and is written into the module's doc comment: **a
mid-stream error is not surfaced by this client.** It is the caller's, because only the caller knows
whether it is proxying the stream onward or consuming it.

## 9. The error model

### 9.1 One shape

```ts
interface PaigasusError {
  presentation: Presentation;              // AC 4 — what the UI does
  domain: ErrorDomain | null;              // parsed; null when absent/unknown
  reason: ErrorReason | null;              // parsed; null when unmapped
  rawReason: string | null;                // what the wire actually said
  message: string;                         // human-readable. NEVER branched on.
  correlationId: string | null;
  requestId: string | null;
  retryable: boolean | null;               // null === the wire's "unknown"
  metadata: Readonly<Record<string, string>>;
  transport: { kind: 'grpc'; code: Code } | { kind: 'http'; status: number };
}

type Presentation =
  | 'relogin'        // UNAUTHENTICATED
  | 'forbidden'      // PERMISSION_DENIED
  | 'not-found'      // NOT_FOUND
  | 'degraded'       // UNAVAILABLE
  | 'invalid-input'  // INVALID_ARGUMENT / 400
  | 'conflict'       // ALREADY_EXISTS / FAILED_PRECONDITION / 409
  | 'generic';       // everything else, incl. every unmapped reason
```

Two structured discriminants, and both are data:

- `presentation` is coarse and derives from the **transport status** — the gRPC `Code` or the HTTP
  status. It is what AC 4 asks for and what a page-level boundary switches on.
- `reason` is fine and derives from the **registry**. It is what a specific message or a field
  highlight uses.

`message` is carried for display and logging and is never an input to any branch. AC 2 is satisfied
structurally: no function in `src/errors/` reads `message`.

`retryable` is tri-state deliberately. The wire's three values are `"true" | "false" | "unknown"`
(`Retryable::as_wire`), and collapsing `unknown` to `false` would silently assert non-retryability
the service declined to assert. ADR-0019 decision 7 exists precisely so clients stop inferring.

### 9.2 The wire-reason codec lives in `@paigasus/proto`, not the SDK

The TypeScript twin of `ErrorReason::{as_wire_reason, from_wire_reason}` goes next to
`capabilityWireKey` in `ts/packages/paigasus-proto/src/error.ts`. That is where the existing
precedent lives, it is a contract concern rather than a transport one, and it keeps the codec
available to any future consumer that is not the SDK.

Both directions are **derived from the descriptor**, never hand-tabulated:
`ErrorReasonSchema.values` yields `DescEnumValue[]` carrying `.name` — the raw proto name, e.g.
`ERROR_REASON_SLUG_CONFLICT`. `capabilityWireKey`'s doc comment already records why `.name` and not
`.localName`: protobuf-es's shared-prefix heuristic (`findEnumSharedPrefix`) degrades for the whole
enum if any value's short name is empty or starts with a digit, and `.name` cannot be affected by
it.

`fromWireReason` reproduces the Rust `is_wire_token` **allow-list** exactly:

```
^[a-z][a-z0-9]*(-[a-z0-9]+)*$
```

checked *before* any case transform. This is not defensive decoration. `String.prototype
.toUpperCase()` folds `ı` (U+0131) to `I` and `ſ` (U+017F) to `S` in JavaScript just as
`str::to_uppercase` does in Rust, so a deny-list check would let `ınternal` and `ſlug-conflict`
reconstruct valid proto names. A lenient TypeScript parser would be laxer than the Rust one and
would weaken AC 3's table test — the exact warning in this issue's handoff comment.

A parity test asserts the TS parser rejects the same ten inputs the Rust test rejects
(`error.rs:287-302`): `slug_conflict`, `SLUG-CONFLICT`, `Slug-Conflict`, `""`, `-slug`, `slug-`,
`slug--conflict`, `no-such-code`, `ınternal`, `ſlug-conflict`.

### 9.3 The presentation table — AC 3

`src/errors/presentation.ts` maps every `ErrorReason` to a `Presentation`, as a
`Record<ErrorReason, Presentation>`. Two things make it self-maintaining:

1. **The type.** A total `Record` over the enum makes a missing reason a **compile** error, not just
   a test failure. Adding a value to `error.proto` and regenerating breaks `tsc` in this file.
2. **The test.** A table test iterates `ErrorReasonSchema.values`, skips the `UNSPECIFIED` sentinel,
   and asserts each remaining reason resolves through `fromWireReason(asWireReason(r))` and has a
   presentation entry. This is AC 3 verbatim: an unmapped code fails a test rather than rendering an
   empty toast.

The type check alone is not enough — a future refactor to `Partial<Record<…>>` or an index signature
would silently switch it off, and the test is what notices. The test alone is not enough either — it
runs late. Both are kept, deliberately.

`presentation` is derived from the transport status (§ 9.1), so what is this table *for*? It is the
override channel: a reason whose transport code is coarser than its meaning. `MISSING_SCOPE` is the
worked example — it is a **500** (`GatewayError::MissingScope`, an internal invariant violation)
despite a name that reads like a client error, a hazard ADR-0019 A1.3 and SMA-504 both flag. The
table maps it to `generic`, not `forbidden`. Where a reason needs no override its entry is the
literal `'from-transport'`, so every one of the 57 is a deliberate, reviewed decision rather than a
default.

### 9.4 Four inputs, one output

`mapError(e: unknown): PaigasusError` accepts:

1. **A `ConnectError`** — `err.findDetails(ErrorInfoSchema)[0]` gives `(reason, domain, metadata)`.
   `metadata` carries `retryable`, and `correlation_id`/`request_id` **when the error was raised
   inside a request scope** (`convert.rs:59-74`) — they are omitted, not nulled, outside one.
   `err.code` gives the gRPC `Code`. `err.metadata` is *"a union of response headers and trailers"*
   (`connect-error.d.ts:22-24`), which is the fallback for the correlation id.
2. **An IAM HTTP response** — body `{error:{code,message}}`. It carries **no** correlation id and
   **no** retryable; both are response *headers*: `paigasus-correlation-id`, `paigasus-request-id`,
   `paigasus-retryable` (`paigasus-observability/src/correlation.rs:20,31-33`). The issue's AC 2
   asks for "a generic fallback plus correlation id", so `mapError` must be handed the headers, not
   only the body. Its HTTP arm therefore takes `{ status, headers, body }`.
3. **A gateway HTTP response** — body `{error:{message,type,param,code}}`. `code` is drawn from the
   same registry (asserted `gateway/adapters/http/error.rs:296-309`). `type` keeps its OpenAI
   semantics and is **not** read: it takes only two values and both are coarser than `code`.
   `param` is carried into `metadata` under the key `param` when present — it is `null` in every
   case except `StreamingDisabled`.
4. **A parsed terminal SSE frame** — the same gateway envelope shape, with no status and no headers.
   `transport` is reported as `{ kind: 'http', status: 200 }`, because the head was already
   committed with 200 when the failure happened. `retryable` is `null`: that frame deliberately
   carries no retryable signal (`chat.rs:56-62`).

**Unknown reason handling (AC 2).** `rawReason` always holds what the wire said. When
`fromWireReason` rejects it, `reason` is `null`, `presentation` falls back to the transport-derived
value, and `correlationId` is preserved. An unrecognized code degrades to a generic presentation
plus a user-reportable id — never to a thrown error and never to an empty message. ADR-0019's own
Consequences call the correlation id "the single highest-value item here for a self-hosted product",
so it survives every fallback path.

**The two system-retirement 409s are not special-cased.** They add sibling keys next to the standard
`error` object (`grants`, `total_surviving`, `truncated`; or `kind`, `source`, `description`) rather
than replacing it (`system_retirement.rs:111-126`). `mapError` reads `error.code` and ignores the
siblings, so it works unchanged; a caller that wants the surviving-grants list reads the body
itself. Inventing a union arm for two endpoint-specific payloads would put endpoint knowledge in the
error mapper.

## 10. Testing

| Tier | What it proves |
|---|---|
| Wire-reason codec parity | Both directions against all 57 reasons; the ten malformed inputs the Rust test rejects |
| Presentation totality (AC 3) | Every descriptor value has an entry; `MISSING_SCOPE` maps to `generic` |
| `mapError` — gRPC | A synthetic `ConnectError` carrying a real `ErrorInfo` detail round-trips to the right `(domain, reason)`; `correlation_id`/`retryable` are read from `metadata` |
| `mapError` — HTTP | Both envelopes; the correlation id comes from **headers**, not the body; tri-state retryable |
| `mapError` — degradation (AC 2) | An unknown reason yields `reason: null`, keeps `rawReason` and `correlationId`, and picks a transport-derived presentation |
| AC 4 mapping | Each of `UNAUTHENTICATED`/`PERMISSION_DENIED`/`NOT_FOUND`/`UNAVAILABLE` yields its state |
| Message-independence (AC 2) | The same wire error with three different `message` strings maps to three identical `PaigasusError`s modulo `message` |
| Transport cache | Same base URL returns the same object; two tokens on one cached transport produce two `authorization` headers and neither is retained |
| Server-only structure (AC 1) | Every `exports` entry imports `./server-guard.js` first, driven off `package.json` |
| Chat | Non-streaming maps a non-2xx; streaming returns the identical `ReadableStream` object it was given |

**No live-service tier.** The SDK has no server to talk to in CI, and standing one up is
SMA-509/SMA-510's integration surface. Transport behaviour is tested through interceptors and a stub
`fetch`, which is honest about what is and is not covered: this suite proves the SDK *forms* correct
requests and *interprets* correct responses. It does not prove IAM accepts them. That gap is stated
rather than papered over.

## 11. Registration obligations

Four edits, three of them gate-enforced:

1. **`ci/affected-graph/run.sh:258-259`** — the `contracts->proto` expected set gains
   `paigasus-sdk-ts`. Strict equality; re-measured, not transcribed (§ 4.2, § 13 M4).
2. **`ci/affected-graph/ci_targets.py:393-400`** — `CONTRACTS_GENERATE_INPUTS` is a **strict-equality**
   pin of `contracts:generate`'s inputs. Adding `contracts/buf.gen.googleapis.yaml` to that task
   reds `repo:affected-smoke` until this tuple is updated. The issue does not mention this; it is a
   real obligation.
3. **`ts/packages/paigasus-sdk/moon.yml`** — `dependsOn: ['paigasus-proto-ts']`, hand-declared,
   because TS edges are not auto-derived (§ 4.2).
4. **`ts/pnpm-workspace.yaml`** — two catalog entries, `@connectrpc/connect` and
   `@connectrpc/connect-node`, both `^2.2.0`, each with the header comment every existing entry
   carries. `@bufbuild/protobuf` is already catalogued at `^2.14.1` and satisfies connect-node's
   `^2.7.0` peer range.

Not required, and checked: none of `SELF_SCHEDULED_GATES`, `SELF_TASK_EXPECTED_GLOBS`,
`REQUIRED_REPO_TASKS`, `T_EXEMPT`, `cargo_moon_parity.py` or `task_inputs.py` holds a Moon
project-id list that a new TS package perturbs — they are keyed on `repo:*` gate names and Rust
crates. This package adds no `repo:*` gate, so `ci.yml`'s `T=(…)` array and the CLAUDE.md
marker-delimited command are untouched.

`ts/pnpm-lock.yaml` changes (two catalog entries plus the workspace edge). No `run.sh` case anchors
on it, so no expected set moves; it re-keys TS tasks generically, which is correct.

### 11.1 Dependency cooldown

`@connectrpc/connect` 2.2.0 published **2026-09-07**, two days ago. pnpm 11's default 24-hour
`minimumReleaseAge` is satisfied, but only just — and npm Dependabot has a 3-day cooldown. If CI
reds on release age, that is the cause and waiting is the fix, not a version change.

## 12. Acceptance criteria mapping

| AC | Where |
|---|---|
| 1 — `import 'server-only'`; a client import fails the build | § 6.2, four layers |
| 2 — branch on `(domain, reason)` only; unknown → generic + correlation id | § 9.1, § 9.4; two tests |
| 3 — table test driven off the registry | § 9.3; total `Record` **and** a descriptor-driven test |
| 4 — the four gRPC codes map to four states; raw statuses never reach the browser | § 9.1 `Presentation`; `PaigasusError` carries no `ConnectError` |
| 5 — `ci/affected-graph/run.sh` updated in the same change | § 4.2, § 11 item 1 — **corrected**: one id, not "every downstream app" |

## 13. Measurements

M1, M2 and M3 are **taken**; M4–M7 are taken during implementation.

| # | Measurement | Status |
|---|---|---|
| M1 | A TS-only template against `buf.build/googleapis/googleapis --path google/rpc/error_details.proto` exits 0 and emits exactly one self-contained file exporting `ErrorInfoSchema` | **TAKEN** — buf 1.70.0, es v2.13.0 |
| M2 | `@connectrpc/connect@2.2.0` exports `createContextKey`/`createContextValues`; `CallOptions.contextValues` exists; the interceptor request carries `readonly contextValues` | **TAKEN** |
| M3 | `findDetails` requires a descriptor — there is no schema-free overload | **TAKEN** |
| M4 | The `contracts->proto` affected set after the edge lands is exactly the seven existing ids plus `paigasus-sdk-ts` — no app enters | pending |
| M5 | Reversing the two `buf generate` calls destroys `google/rpc/error_details_pb.ts`, and the assertion test catches it | pending |
| M6 | Two `getTransport` calls with one base URL return the same object; two tokens on it produce two `authorization` headers | pending |
| M7 | `String.prototype.toUpperCase()` folds `ı`→`I` and `ſ`→`S` in Node 24, so the allow-list is load-bearing in TS exactly as in Rust | pending |

## 14. Out of scope

- **Wiring any app to the SDK.** No `package.json` in `ts/apps/` gains `@paigasus/sdk`. That is
  SMA-509 (capability discovery) and SMA-510 (app shell).
- **`IntrospectPrincipalResolver`.** SMA-506 § 17 names this issue as where it lands, but the SDK's
  boundary rule bans importing `@paigasus/auth`, and SMA-506 is unmerged. The SDK exports the typed
  `AuthnService` client; the *app* wires it into `@paigasus/auth`'s structurally-typed
  `PrincipalResolver` port. Neither package imports the other. Flagged for SMA-506's author rather
  than resolved unilaterally here.
- **Widening `ci/error-registry/check.py` to TypeScript.** It is Rust-only and scoped to `rs/crates`.
  AC 3's descriptor-driven test covers the consumed side for this package. A repo-wide two-way TS
  gate is SMA-507's deferred AC 2 and deserves its own issue.
- **A retry or backoff policy.** `retryable` is carried, not acted on. Deciding *when* to retry is
  a caller policy and belongs with the caller that knows whether the operation is idempotent.
- **grpc-web or any browser-direct transport.** ADR-0018 decision 2, explicitly.
