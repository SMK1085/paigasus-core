# SMA-575 — close the remaining gaps for the IAM ops surfaces in `@paigasus/sdk`

**Issue:** SMA-575 (split out of SMA-501).
**ADR:** ADR-0018 (Connect-ES over gRPC; no OpenAPI surface), Amendment A1 (SMA-508).
**Related specs:** `2026-09-09-sma-508-sdk-design.md` (§ 4.1, § 4.2), `2026-08-22-sma-501-iam-ops-proto-services-design.md`.
**Path:** bounded. The code exists. This change adds proof, corrects documents, and removes one
source of test flakiness that the new proof would otherwise hit.
**Revision 2** (2026-09-16): the Stage 2 challenge findings are folded in. § 10 lists them.

## 1. Why this spec is small

The issue was written on 2026-08-22, when `@paigasus/sdk` was an empty stub. SMA-624, SMA-508 and
SMA-625 then built the package. The state on `main` at `43afaf4a` is:

| AC | State | Evidence |
|---|---|---|
| 1. The three ops surfaces are reachable over Connect-ES, with no hand-written DTOs | Built, **not proven** | `src/iam.ts:11` imports `UserService`, `AuthorizationService` and `OutboxService` from `@paigasus/proto/iam`. `createIamClient` is generic over `DescService`. No test names one of the six ops RPCs. |
| 2. A hand-written HTTP client covers only the gateway chat surface | True, **not enforced** | The only `fetch` call in `src/` is in `src/chat.ts`. Nothing fails if a second file adds one. |
| 3. `contracts->proto` has a new baseline, and `moon ci` is green | **Done** | `ci/affected-graph/run.sh:288-289` lists 13 ids, `paigasus-sdk-ts` and the three SDK consumers included. |
| 4. ADR-0018 § 5 matches the code | **Partly done, and partly stale** | Amendment A1.1 shrinks decision 5 to the chat endpoint. See § 5 for the stale parts. |

This spec closes AC-1, AC-2 and AC-4. AC-3 needs no change. It is verified again by the full
`moon ci` run before the PR.

### 1.1 Two errors in the issue text

- `CreateUser` is on **`UserService`** (`iam.proto:534-536`), not on `TenancyService`.
- The six ops RPCs are: `UserService.CreateUser`, `AuthorizationService.RetireSystemPolicy`, and
  `OutboxService.{ListDeadLetters, ReplayDeadLetter, BulkReplayDeadLetters, DiscardDeadLetter}`
  (`iam.proto:366`, `:535`, `:683-686`).

A Linear comment records both. The issue description is not edited.

## 2. Precondition — no test writes into `src/`

`tests/server-guard.test.ts:112-235` has six probe tests. Each one writes a file such as
`src/__unrelated-import-probe.ts` into `src/`, runs the directory walk, and deletes the file in a
`finally`. Vitest 5 runs test files in parallel by default, and `vitest.config.ts` does not change
that. The new guard in § 4 walks `src/` too. It could see a probe file and report a false violation
(the `__unrelated-import-probe.ts` probe imports `node:fs`, which is not on the allowlist), or fail
with `ENOENT` when the probe is deleted between the list and the read. A killed run also leaves a
probe in `src/` permanently.

**Change:** each of the six probe tests calls the existing pure function
`importsServerOnly(content, fileName)` with its content string directly. No test writes a file.
The assertions keep their meaning: each probe asserts that the function flags, or does not flag,
the content. The `writeFileSync` and `unlinkSync` imports are removed.

**Rule, recorded in a comment at the top of `tests/http-surface.test.ts` and
`tests/server-guard.test.ts`:** no test in this package writes into `src/`.

## 3. AC-1 — a test that proves the ops surfaces are reachable

**Changed file:** `ts/packages/paigasus-sdk/tests/iam-factory.test.ts`.

That file already mocks `@connectrpc/connect-node` so that the real `getTransport` call reaches a
recording `Transport`, and it drives the **production** factory `createIamClient`. The new tests go
into this file, so no third copy of the recorder is made. The recorder is extended: for each call it
records the method descriptor, the input and the `ContextValues`. The existing FIX 1 test keeps its
assertions.

### 3.1 The row table

The table has six rows. Each row holds:

- `service`: the expected `typeName`, as a string literal (for example
  `'paigasus.iam.v1.OutboxService'`);
- `rpc`: the expected RPC name, as a string literal;
- `request` and `response`: the expected message `typeName` strings;
- `call`: a closure with its own type, for example
  `(auth) => createIamClient(OutboxService, OPTIONS, auth).listDeadLetters(INIT)`.

The closure keeps each call fully typed. An `it.each` that indexes `client[row.method]` over
different services gives a union with no common keys, which needs casts and fails
`@typescript-eslint/no-unsafe-*`.

### 3.2 Assertions

1. **Each row is callable through `createIamClient`.** For each row, the test runs `call(auth)` once
   and asserts on the one recorded call:
   - `method.parent.typeName` equals the row's `service` string;
   - `method.name` equals the row's `rpc` string;
   - `method.input.typeName` and `method.output.typeName` equal the row's `request` and `response`
     strings. This is the runtime proof that the request and response types come from the contract;
   - the recorded input equals the init object that the closure passed (the plan measures whether
     Connect passes it through by identity or by value, and asserts the measured form);
   - the recorded `ContextValues` carries the `Auth` that was given to `createIamClient`.
2. **The table is complete.** The set of `(service, rpc)` pairs in the rows equals the pinned set of
   six pairs. The `OutboxService` part of the pinned set is derived from `OutboxService.methods`, so
   a new Outbox RPC needs a new row. A floor asserts `rows.length === 6`.
3. **The Outbox service is pinned by strict equality.** The names in `OutboxService.methods` equal
   the four expected names. `OutboxService` exists only for the ops surfaces, so a change to it is a
   change to this issue's scope. `UserService` has one RPC today, and `AuthorizationService` has
   many that are not ops surfaces. For those two, row assertions already prove the one RPC that
   matters, and no strict set is pinned, so unrelated contract work does not red this test.
4. **Type-level proof for the factory path.** For each of the six methods, `expectTypeOf` asserts
   that the first parameter equals `MessageInitShape` of the generated request schema, and that the
   resolved return type equals `MessageShape` of the generated response schema. `expectTypeOf` from
   vitest re-exports `expect-type`, whose `toEqualTypeOf` fails plain `tsc`
   (`ts/packages/paigasus-auth/tests/core/session.test.ts:76-79` relies on the same behaviour).
5. **The `./iam` entry exports nothing else.** The runtime keys of `src/iam.ts` equal a pinned
   list by strict equality, and the type-only exports of `src/iam.ts` (read with the TypeScript
   AST) equal a pinned list. A new exported wrapper with its own DTO, such as
   `createUser(dto: { email: string })`, then fails this test.

### 3.3 `typecheck` must key on the tests

The inherited `typecheck` and `build` inputs (`.moon/tasks/typescript-project.yml:28-45`) are
`@group(sources)` and config files. They do not include `tests/**`. A PR that edits only a test
file therefore gets a cached `typecheck` PASS, and assertion 4 is never checked in CI.

**Change:** `ts/packages/paigasus-sdk/moon.yml` appends `'tests/**/*'` and `'vitest.config.ts'` to
the `inputs` of `typecheck` and of `build`. Both tasks run the same `tsc -p tsconfig.json` command,
and `tsconfig.json` `include` names both paths. The append form is kept (no `merge: replace`). The
plan measures whether any `ci/affected-graph/run.sh` case changes, and re-baselines a case in the
same change if one does.

### 3.4 Proof that the assertions bite

The plan measures one mutation for each assertion, and records the assertion message that fails:

1. change the expected `service` string in one row: assertion 1 fails on `parent.typeName`;
2. change the expected `rpc` string in one row: assertion 1 fails on `method.name`;
3. make `createIamClient` drop `auth` (the FIX 1 mutation): assertion 1 fails on the context value;
4. delete one row: assertion 2 fails;
5. add a fifth name to the pinned Outbox set: assertion 3 fails;
6. change one `expectTypeOf` target to a different request schema: `tsc` fails. Measure with
   `pnpm exec tsc -p tsconfig.json --noEmit` or `moon run … --force`, because a Moon cache hit
   would replay a PASS;
7. add `export const createUser = …` to `src/iam.ts`: assertion 5 fails.

Each mutation is restored by an Edit of the marked text, not by `git checkout --`, which also
reverts uncommitted work. After any fix, the whole battery runs again.

## 4. AC-2 — a guard that allows hand-written HTTP only for the chat surface

**New file:** `ts/packages/paigasus-sdk/tests/http-surface.test.ts`.

### 4.1 Why a vitest test and not an ESLint rule

- Nobody can switch off a vitest assertion with an `eslint-disable` comment.
- Its negative controls are plain unit tests of a pure function, which is simpler to prove than a
  custom lint rule.
- The package already uses TypeScript-AST tests for structural rules (`tests/server-guard.test.ts`).
- A new `no-restricted-imports` block for the same files would **replace** the
  `paigasus/boundaries/sdk` block in flat config (`eslint.mjs:413-415`). Other rule names do not
  have this problem, but the three reasons above are enough.

The test runs in `paigasus-sdk-ts:test`, whose inputs already include `src/**` and `tests/**`.

### 4.2 The checker

The checker is one pure function: `findViolations(relPath, source)`. `relPath` is the path relative
to the package root, with `/` separators. The function parses `source` with the TypeScript
compiler API and visits nodes with `ts.forEachChild`. It does not use `getChildren()`, because that
includes JSDoc nodes, and a `{@link fetch}` in a doc comment must not count.

It applies these rules:

1. **Import allowlist.** A module specifier appears in an `import` (value or type), an
   `export … from` (value or type), a dynamic `import()` and a `require()`. Each specifier must be
   a string literal. A variable or a template literal is a violation. Each literal must be one of:
   - `server-only`, `@connectrpc/connect`, `@bufbuild/protobuf`, `@paigasus/proto`,
     `@paigasus/proto/iam`;
   - `@connectrpc/connect-node`, **only** in `src/transport.ts`, and **only** with the named
     imports `createGrpcTransport` and `Http2SessionManager`. These two names are gRPC transport
     plumbing. This package also exports a generic HTTP client (`createNodeHttpClient`) and the
     Connect and gRPC-web transports, so a general allow would let a new file send REST calls;
   - a relative specifier (`./`, `../`) that resolves to a path inside `src/`.
2. **Network and escape identifiers.** Outside `src/chat.ts`, an identifier named `fetch`,
   `XMLHttpRequest`, `WebSocket`, `EventSource`, `Request`, `Response`, `RequestInit`,
   `globalThis`, `global` or `process` is a violation. The last three close the obvious escapes:
   `globalThis['fe' + 'tch']` and `process.getBuiltinModule('node:https')`. `Headers` is not on
   the list, because `src/errors/map-error.ts` uses it. The rule is by identifier name, so a local
   variable named `fetch` also fails. That is intentional: the file list is small, and a false
   positive costs one rename. The exemption matches `relPath === 'src/chat.ts'` exactly, so
   `src/errors/chat.ts` is not exempt.
   **Correction (final review, SMA-575).** The exemption for `src/chat.ts` is now per identifier
   NAME, not per file. Only `fetch`, `globalThis` and `Response` are exempt there. These are the
   three names `src/chat.ts` uses, MEASURED with the TypeScript compiler API. Every other banned
   identifier fails in `src/chat.ts` too.
3. **The raw HTTP/2 session escape.** `Http2SessionManager` (allowed by rule 1, in
   `src/transport.ts` only) declares `request(method, path, headers, options)` and `connect()`.
   Both send a raw HTTP/2 request over the object's own session. Both bypass every generated
   Connect-ES client. In every file, `src/chat.ts` included, a property access whose name is
   `request` or `connect` is a violation, on any object. The check is by property NAME, not by
   the object's declared type. MEASURED: no such property access exists in `src/` today.

`src/chat.ts` is still subject to rule 1 and rule 3.

### 4.3 What the test asserts over the real tree

1. **No violations.** The walk covers every file under `src/`. A file with an extension other than
   `.ts` is a violation, so `.mts`, `.cts`, `.tsx` or `.js` files cannot hide from the walk.
2. **The walk is not empty.** It finds at least the ten files that exist today (six in `src/`, four
   in `src/errors/`).
3. **Every entry point is in the walk.** Every target in `package.json` `exports` starts with
   `./src/`.
4. **The chat exception is live and narrow.** `src/chat.ts` contains exactly one call through the
   fetch seam (`fetchImpl(`, `src/chat.ts:239`). Its only string or template literal that contains
   `/v1/` is the `/v1/chat/completions` path (`src/chat.ts:206`). So `chat.ts` cannot gain a second
   endpoint without a red test. This check reads the AST, not the text.
5. **No direct fetch call (final review, SMA-575).** The number of `CallExpression` nodes in
   `src/chat.ts` whose callee is the identifier `fetch`, or the property access
   `globalThis.fetch`, is 0. `src/chat.ts` reads `globalThis.fetch` once, as a value into
   `fetchImpl`, and calls only `fetchImpl` after that. This check reads the AST, not the text.
   The check unwraps parentheses around the callee, so `(fetch)(u)` also counts as a direct call.

### 4.4 Negative controls

The test drives `findViolations` with in-memory fixtures. Each fixture is a code string in an array,
not a comment, so comment removal cannot make it inert. Each one must yield exactly the stated
number of violations:

| Fixture | relPath | Violations |
|---|---|---|
| `fetch(url)` | `src/x.ts` | 1 |
| `globalThis.fetch` | `src/x.ts` | 2 (both identifiers count) |
| `type F = typeof fetch` | `src/x.ts` | 1 |
| `globalThis['fe' + 'tch']` | `src/x.ts` | 1 |
| `process.getBuiltinModule('node:https')` | `src/x.ts` | 1 |
| `new WebSocket(u)` | `src/x.ts` | 1 |
| `(r: Response) => r` | `src/x.ts` | 1 |
| `import 'node:https'` | `src/x.ts` | 1 |
| `import type { Dispatcher } from 'undici'` | `src/x.ts` | 1 |
| `export * from 'undici'` | `src/x.ts` | 1 |
| `export type { X } from 'undici'` | `src/x.ts` | 1 |
| `await import('node:http')` | `src/x.ts` | 1 |
| ``await import(`undici`)`` | `src/x.ts` | 1 |
| `await import(name)` | `src/x.ts` | 1 |
| `require('axios')` | `src/x.ts` | 1 |
| `import { createNodeHttpClient } from '@connectrpc/connect-node'` | `src/transport.ts` | 1 |
| `import { createGrpcTransport } from '@connectrpc/connect-node'` | `src/x.ts` | 1 |
| `import { x } from '../../paigasus-auth/src/x'` | `src/x.ts` | 1 |
| `// fetch is mentioned here` and `/** {@link fetch} */ export const a = 1` | `src/x.ts` | 0 |
| `const r = await fetch(u)` | `src/chat.ts` | 0 |
| `import 'node:https'` | `src/chat.ts` | 1 |
| `process.getBuiltinModule('node:https')` (final review, SMA-575) | `src/chat.ts` | 1 |
| `new WebSocket(u)` (final review, SMA-575) | `src/chat.ts` | 1 |
| `import { fetch } from './x'` | `src/errors/chat.ts` | 1 |
| only allowlisted specifiers | `src/x.ts` | 0 |
| `new Http2SessionManager(u); s.request('POST', '/v1/users', {}, {})` (final review, SMA-575) | `src/transport.ts` | 1 |
| `(m: Http2SessionManager) => m.connect()` (final review, SMA-575) | `src/transport.ts` | 1 |
| `import cn, { createGrpcTransport } from '@connectrpc/connect-node'` (final review, SMA-575) | `src/transport.ts` | 1 |
| `import * as cn from '@connectrpc/connect-node'` (final review, SMA-575) | `src/transport.ts` | 1 |

The plan also measures one mutation for each real-tree assertion: `fetch` added to `src/iam.ts`
(rule 2), `import 'node:https'` added to `src/transport.ts` (rule 1), the `fetchImpl(` call removed
from `src/chat.ts` (assertion 4), and the walk root pointed at an empty directory (assertion 2).
The final review adds one more, for assertion 5: `fetchImpl(url, {` changed to `fetch(url, {` in
`src/chat.ts` fails both assertion 4 and assertion 5.

### 4.5 Limits (stated, not closed)

- The guard sees only `src/`. A hand-written client in `tests/` is not a shipped surface.
- A library on the allowlist can still do HTTP. `@connectrpc/connect` and the two allowed
  `connect-node` names are gRPC transport plumbing. **Corrected (final review, SMA-575):** the raw
  HTTP/2 session methods behind those two names, `Http2SessionManager.request` and `.connect`, are
  closed by name under rule 3 above. The guard controls *hand-written* HTTP, which is what AC-2
  asks for.
- An allowlist entry can change without review of what the new module does. The list is in one
  place and is small, so the diff shows it.
- `eval`, `new Function(…)`, and a string passed to a library that evaluates code are not detected.
  Each one is an obvious red flag in review.
- The `/v1/` check in assertion 4 reads a single string or template literal. It does not see a
  split literal, for example `'/v' + '1/users'`.
- Rule 3, the property-name check, does not see an aliased or element-access call. For example
  `const { request } = s; request(...)` or `s['request'](...)` are not caught. The allowlist is
  small, so a reviewer can still find these by hand.
- Assertion 5 of § 3.2 pins the `./iam` entry only. The `./chat`, `./errors` and root entries have
  their own existing tests (`tests/index-barrel.test.ts`, `tests/chat.test.ts`), which do not pin
  exact export sets.

## 5. AC-4 — ADR-0018 Amendment A2

ADR-0018 is in Notion. It was read on 2026-09-16 (decision 5 and Amendment A1 in full). The
relevant text is:

- Decision 5: "**HTTP-only surfaces get a small hand-written fetch client** in `@paigasus/sdk/http`:
  `/v1/users`, `/v1/outbox/dead-letters/*`, `/v1/authz/system-policies/{id}/retire`, and the
  gateway's OpenAI-compatible chat endpoint."
- A1.1: "**The decision itself is unchanged** — HTTP-only surfaces still get a small hand-written
  fetch client in `@paigasus/sdk/http`. Only its membership shrinks, to the gateway's chat endpoint
  alone."
- A1.3: "The second half is currently **vacuous**: measured, no project in `ts/` declares a
  `dependsOn` on `paigasus-sdk-ts` or lists `@paigasus/sdk` in a `package.json`, so the delta is
  exactly one id. The clause becomes live only when SMA-509 or SMA-510 wires an app to the SDK."

Two facts are stale:

1. **The subpath.** The package has no `./http` entry. The chat client is `@paigasus/sdk/chat`
   (`package.json` `exports`). This was already wrong on 2026-09-09: the SMA-508 spec
   (`2026-09-09-sma-508-sdk-design.md:130`) names `@paigasus/sdk/chat`. So this is an erratum.
2. **The downstream clause is live.** `iam-console`, `gateway-console` and `@paigasus/console-core`
   now list `@paigasus/sdk` in their `package.json`. SMA-511 and SMA-512 added them to the
   `contracts->proto` case.

The A1.1 line references (`iam.proto:521-536`, `:613-687`, `:356-367`, `mod.rs:87`, `:94`, `:101`)
were checked on 2026-09-16 and still match. They need no correction.

**Decision: append Amendment A2, and do not rewrite A1.** A1 is an accepted record. A2 corrects it
and states its date. One line is added under the A1 heading: "Partly corrected by A2
(2026-09-16)." No other A1 text changes. Decision 5 itself is not edited, because A1 and A2 already
say how to read it.

**A2 content** (in Simplified Technical English):

- **A2.1 — Erratum: the chat client subpath.** Decision 5 and A1.1 name `@paigasus/sdk/http`. The
  built subpath is `@paigasus/sdk/chat`, and there is no `/http` entry. The SMA-508 spec already
  used `/chat`. The membership of decision 5 stays as A1.1 says: the gateway chat endpoint only.
- **A2.2 — The downstream clause is now live.** Name the three consumers and the issues that added
  them. Refer to the `contracts->proto` case in `ci/affected-graph/run.sh` by name. Do not copy the
  id list into the ADR, because a copied list goes stale (the six-id list in the ADR's Consequences
  section did).
- **A2.3 — Two controls now hold decision 5.** Name the proof in `tests/iam-factory.test.ts`
  (AC-1) and the guard in `tests/http-surface.test.ts` (AC-2), with the PR link. The file names
  are checked again at merge time.
- A status line in the A1 form: amendment accepted, ADR status unchanged.

The plan also checks the ADR index table in Notion. A2 changes no status, so the table is expected
to need no edit. The plan records what it finds.

**When:** the Notion edit is made after the PR is open, so A2.3 can link the PR. It is made before
GATE 2 of the pipeline (the stop after the CodeRabbit loop, where Sven decides to merge), so the PR
description can say that AC-4 is done.

## 6. One stale comment in the package

`ts/packages/paigasus-sdk/package.json` `_comment_entries` says "Two entry points when this package
is finished". The `exports` map has five. The comment is corrected to describe the five entries and
to name `./chat` as the only hand-written HTTP surface, with a pointer to
`tests/http-surface.test.ts`.

## 7. What does not change

- No change to `src/`. The runtime behaviour of the package does not change.
- No new project edge, so the `contracts->proto` set stays at 13 ids. A task-case baseline in
  `ci/affected-graph/run.sh` changes only if § 3.3 measures that it must.
- No new `repo:*` gate, so no registry obligations apply.
- No console code. The ops-console screens are the real consumer. They do not exist yet, and this
  change does not build them. `@paigasus/console-core`'s `IamClients` gets no `user` or `outbox`
  entry here.

## 8. Existing controls this change relies on

- `run_task_case_ci "proto-iam->sdk"` (`ci/affected-graph/run.sh:488-489`) proves that an edit to
  the generated `iam_pb.ts` selects `paigasus-sdk-ts:test`. So a contract change re-runs the new
  tests.
- `tests/server-guard.test.ts` keeps its entry-point rule.

## 9. Verification

- `moon run paigasus-sdk-ts:test paigasus-sdk-ts:typecheck ts:lint ts:fmt`.
- The mutations in § 3.4 and § 4.4, each measured red and then restored.
- The full CI target list from CLAUDE.md, with `--base origin/main`. The local-bash limits in
  CLAUDE.md apply: `repo:actionlint` has no working local bash, so its result comes from CI. If the
  plan quotes the moon diagnosis procedure, it carries the `moon-diagnosis` marker (check 12).

## 10. Challenge log (Revision 2)

**Folded in:** the parallel-probe race (§ 2); the false line-reference correction (removed, § 5);
`typecheck` not keyed on tests (§ 3.3); the row-table completeness pin (§ 3.2 item 2); mutations
that show the named assertion catches the error (§ 3.4); the `connect-node` per-file allowlist
(§ 4.2); the syntax escapes (§ 4.2 rules 1-2, § 4.5); the chat-surface narrowing (§ 4.3 item 4);
the `./iam` export pin (§ 3.2 item 5); the wrong `UserService` reason; reuse of the existing
recorder; the corrected ESLint reason; `ts.forEachChild`; the extra fixtures; the exact path match;
relative specifiers inside `src/`; the scan scope; the per-row closure; the recorded input; the
production mutations; no copied id list in the ADR; the erratum reason; the named
`proto-iam->sdk` control; the GATE 2 definition; the check-12 marker; the verbatim ADR quotes; the
ADR index check.

**Not folded in:**

- *Re-export generated message types from `@paigasus/sdk/iam` for future console code.* No consumer
  exists. The ops-console issue decides the shape it needs. This is recorded as an open question.
- *Add `user` and `outbox` to `console-core`'s `IamClients`.* AC-1 asks for reachability from
  `@paigasus/sdk`. The console wiring belongs to the ops-console work.
- *An in-process gRPC server test.* The wire path and the `Authorization` header already have
  tests (`tests/transport-wiring.test.ts`, `tests/iam.test.ts`), and nothing in this change touches
  them. The new tests prove the descriptors and the auth binding for the six RPCs, which is what
  AC-1 needs.
