# SMA-575 — close the remaining gaps for the IAM ops surfaces in `@paigasus/sdk`

**Issue:** SMA-575 (split out of SMA-501).
**ADR:** ADR-0018 (Connect-ES over gRPC; no OpenAPI surface), Amendment A1 (SMA-508).
**Related specs:** `2026-09-09-sma-508-sdk-design.md` (§ 4.1, § 4.2), `2026-08-22-sma-501-iam-ops-proto-services-design.md`.
**Path:** bounded. The code exists. This change adds proof and corrects documents.

## 1. Why this spec is small

The issue was written on 2026-08-22, when `@paigasus/sdk` was an empty stub. SMA-624, SMA-508 and
SMA-625 then built the package. The state on `main` at `43afaf4a` is:

| AC | State | Evidence |
|---|---|---|
| 1. The three ops surfaces are reachable over Connect-ES, with no hand-written DTOs | Built, **not proven** | `src/iam.ts:11` imports `UserService`, `AuthorizationService` and `OutboxService` from `@paigasus/proto/iam`. `createIamClient` is generic over `DescService`. No test names one of the six ops RPCs. |
| 2. A hand-written HTTP client covers only the gateway chat surface | True, **not enforced** | The only `fetch` call in `src/` is in `src/chat.ts`. Nothing fails if a second file adds one. |
| 3. `contracts->proto` has a new baseline, and `moon ci` is green | **Done** | `ci/affected-graph/run.sh:288-289` lists 13 ids, `paigasus-sdk-ts` and the three SDK consumers included. |
| 4. ADR-0018 § 5 matches the code | **Partly done, and partly stale** | Amendment A1.1 shrinks decision 5 to the chat endpoint. See § 4 for the stale parts. |

This spec closes AC-1, AC-2 and AC-4. AC-3 needs no change. It is verified again by the full
`moon ci` run before the PR.

### 1.1 Two errors in the issue text

- `CreateUser` is on **`UserService`** (`iam.proto:534-536`), not on `TenancyService`.
- The six ops RPCs are: `UserService.CreateUser`, `AuthorizationService.RetireSystemPolicy`, and
  `OutboxService.{ListDeadLetters, ReplayDeadLetter, BulkReplayDeadLetters, DiscardDeadLetter}`
  (`iam.proto:366`, `:535`, `:683-686`).

A Linear comment records both. The issue description is not edited.

## 2. AC-1 — a test that proves the ops surfaces are reachable

**New file:** `ts/packages/paigasus-sdk/tests/ops-surfaces.test.ts`.

It uses the pattern of `tests/iam-factory.test.ts`: `vi.mock('@connectrpc/connect-node')` sends the
real `getTransport` call to a recording `Transport`. The test then drives the **production** factory
`createIamClient`, not `bindAuth` directly. A separate file is necessary because `vi.mock` applies to
the whole file (the reason `iam-factory.test.ts` records at its lines 12-15).

The recorder in this file records more than the existing two recorders do. For each call it records
the method descriptor, the input message and the `ContextValues`.

### 2.1 Assertions

1. **Each of the six RPCs is callable through `createIamClient`.** A table of six rows drives one
   `it.each`. Each row names the service, the client method (`createUser`, `retireSystemPolicy`,
   `listDeadLetters`, `replayDeadLetter`, `bulkReplayDeadLetters`, `discardDeadLetter`) and the
   expected wire names. For each row, the test calls the method once and asserts:
   - the recorded `method.parent.typeName` is the expected service (for example
     `paigasus.iam.v1.OutboxService`);
   - the recorded `method.name` is the expected RPC name;
   - the recorded `method.input.typeName` and `method.output.typeName` are the generated message
     names (for example `paigasus.iam.v1.CreateUserRequest`). This is the runtime proof that the
     request and response types come from the contract;
   - the recorded `ContextValues` carries the `Auth` that was given to `createIamClient`. This makes
     sure the ops clients get the same token binding as every other IAM client.
2. **The set of RPCs is pinned by strict equality.** `OutboxService.methods` names equal the four
   names above, in any order. `UserService.methods` and the `AuthorizationService` method list must
   **contain** `CreateUser` and `RetireSystemPolicy`. Only `OutboxService` gets strict equality,
   because it exists only for these ops surfaces. The other two services carry unrelated RPCs, and
   strict equality there would red this test on unrelated contract work.
3. **Type-level proof of "no hand-written DTOs".** For each of the six methods, `expectTypeOf`
   asserts that the first parameter of the client method equals `MessageInitShape` of the generated
   request schema, and that the resolved return type equals `MessageShape` of the generated response
   schema. `tests/**/*` is in the package `tsconfig.json` `include`, so `paigasus-sdk-ts:typecheck`
   fails at compile time when a type differs.

### 2.2 Proof that the assertions bite

The plan records one mutation for each assertion group, with the measured result:

- replace `OutboxService` with `TenancyService` in one row: group 1 fails;
- add a fifth name to the pinned Outbox set: group 2 fails;
- change one `expectTypeOf` target to a different request schema: `typecheck` fails;
- make `createIamClient` drop `auth` (the FIX 1 mutation from `iam-factory.test.ts`): the context
  assertion in group 1 fails.

The plan restores each mutation by an Edit of the marked text, not by `git checkout --`. The
checkout form also reverts uncommitted work.

## 3. AC-2 — a guard that allows a hand-written HTTP client only in `chat.ts`

**New file:** `ts/packages/paigasus-sdk/tests/http-surface.test.ts`.

### 3.1 Why a vitest test and not an ESLint rule

- In flat config, a second `no-restricted-imports` block that matches the same files **replaces**
  the first. A new block for `packages/paigasus-sdk/src/**` would silently switch off
  `paigasus/boundaries/sdk` (CLAUDE.md, the Turbopack gotcha; `eslint.mjs:413-418`).
- The package already uses TypeScript-AST tests for structural rules (`tests/server-guard.test.ts`).
  This guard follows that pattern. It runs in `paigasus-sdk-ts:test`, and its inputs are already
  `src/**` and `tests/**`.

### 3.2 What the guard checks

The test walks every `.ts` file under `src/` and parses it with the TypeScript compiler API. It
does not match text, so a comment that names `fetch` does not count.

1. **Import allowlist, strict.** Every module specifier in an `import`, an `export … from`, a
   dynamic `import()` and a `require()` must be one of:
   `server-only`, `@connectrpc/connect`, `@connectrpc/connect-node`, `@bufbuild/protobuf`,
   `@paigasus/proto`, `@paigasus/proto/iam`, or a relative path (`./`, `../`).
   An allowlist is stronger than a denylist: a new HTTP library (`undici`, `node:https`, `axios`,
   `@connectrpc/connect-web`, …) fails without a list of names. A new dependency needs a deliberate
   edit to this list. This matches the strict-equality style of the repository gates.
2. **Network globals outside `chat.ts`.** An identifier named `fetch`, `XMLHttpRequest`,
   `WebSocket` or `EventSource`, in any file other than `src/chat.ts`, fails the test. This covers
   `fetch(...)`, `globalThis.fetch`, and `typeof fetch`. The rule is by identifier name, so a local
   variable named `fetch` also fails. That is intentional: the file list is small, and a false
   positive costs one rename.
3. **The allowed exception is live.** `src/chat.ts` must contain at least one `fetch` identifier.
   If `chat.ts` moves or stops using `fetch`, the test fails, so the exception cannot point at
   nothing.
4. **The scan is not empty.** The walk must find at least the ten files that exist today (six in `src/`,
   four in `src/errors/`). A
   broken glob then fails the test and does not pass with zero files.

### 3.3 Negative controls

The checker is one pure function, `findViolations(fileName, source)`. The test drives it with
in-memory fixtures, one for each shape: a bare `fetch()` call, `globalThis.fetch`, `typeof fetch`,
`import 'node:https'`, `export * from 'undici'`, `await import('node:http')`, `require('axios')`,
and a `new WebSocket(...)`. Each fixture must yield exactly one violation. One more fixture puts
`fetch` in a comment and must yield none. These fixtures are code strings in arrays, not comments,
so comment removal cannot make them inert.

### 3.4 Limits (stated, not closed)

- The guard sees only `src/`. A hand-written client in `tests/` is not a shipped surface.
- A library on the allowlist can still do HTTP. `@connectrpc/connect-node` does exactly that, over
  gRPC. The guard controls *hand-written* HTTP, which is what AC-2 asks for.
- An import allowlist entry can change without review of what the new module does. The list is in
  one place and is small, so the diff shows it.

## 4. AC-4 — ADR-0018 Amendment A2

ADR-0018 is in Notion. Amendment A1 (2026-09-09, SMA-508) has three stale facts today:

1. A1.1 says the chat client lives in **`@paigasus/sdk/http`**. The package has no `./http` entry.
   The chat client is **`@paigasus/sdk/chat`** (`package.json` `exports`).
2. A1.3 says the rule "the expected set must gain every app downstream" is **vacuous**, because no
   project depends on the SDK. That is no longer true. `iam-console`, `gateway-console` and
   `@paigasus/console-core` depend on `@paigasus/sdk`. The `contracts->proto` set now has 13 ids,
   and SMA-511 and SMA-512 added the downstream ids.
3. A1.1 gives line references as of 2026-09-09 (`iam.proto:521-536`, `:613-687`, `:356-367`,
   `mod.rs:94`, `:101`, `:87`). Several no longer match.

**Decision: append Amendment A2, and do not rewrite A1.** A1 is an accepted record of what was true
on 2026-09-09. A2 corrects it and states its date. One line is added under the A1 heading:
"Partly corrected by A2 (2026-09-16)." No other A1 text changes.

**A2 content** (in Simplified Technical English, like the other amendments):

- **A2.1 — The chat client subpath.** Decision 5 and A1.1 name `@paigasus/sdk/http`. The built
  subpath is `@paigasus/sdk/chat`. There is no `/http` entry. The membership of decision 5 stays as
  A1.1 says: the gateway chat endpoint only.
- **A2.2 — The downstream clause is now live.** Give the 13-id set and name the three consumers.
- **A2.3 — Current references.** Give the current `iam.proto` and `mod.rs` lines, and say that
  line references are a snapshot.
- **A2.4 — Two controls now hold decision 5.** Name `tests/ops-surfaces.test.ts` (AC-1) and
  `tests/http-surface.test.ts` (AC-2), with the PR link.
- A status line in the A1 form: amendment accepted, ADR status unchanged.

The Notion edit is made after the PR is open, so A2.4 can link the PR. It is made before GATE 2, so
the PR description can say that AC-4 is done.

## 5. One stale comment in the package

`ts/packages/paigasus-sdk/package.json` `_comment_entries` says "Two entry points when this package
is finished". The `exports` map has five. The comment is corrected to describe the five entries and
to name `./chat` as the only hand-written HTTP surface, with a pointer to
`tests/http-surface.test.ts`.

## 6. What does not change

- No change to `src/`. The runtime behaviour of the package does not change.
- No change to `ci/affected-graph/run.sh`. No new project edge is made, so the 13-id set stays.
- No change to `moon.yml`. The inherited `test` inputs already include `tests/**/*`.
- No new `repo:*` gate, so no registry obligations apply.
- No console code. The issue says the ops-console screens are the real consumer. They do not exist
  yet, and this change does not build them.

## 7. Verification

- `moon run paigasus-sdk-ts:test paigasus-sdk-ts:typecheck ts:lint ts:fmt`.
- The four mutations in § 2.2 and one mutation for § 3 (add `fetch` to `src/iam.ts`), each measured
  red and then restored.
- The full CI target list from CLAUDE.md, with `--base origin/main`. The local-bash limits in
  CLAUDE.md apply: `repo:actionlint` has no working local bash, so its result comes from CI.
