# SMA-629 — the `iam.deadletters` capability key and the `/iam/dead-letters` screen

- **Issue:** SMA-629 (split from SMA-511, decision D1).
- **Status:** revision 1, approved in chat by Sven on 2026-09-19. Stage 2 challenge pending.
- **Parent spec:** `docs/superpowers/specs/2026-09-11-sma-511-iam-console-design.md` (§ 2.2, § 5.2,
  § 5.3, § 6.3, § 6.6, § 9.4, § 11).

---

## 1. Problem

SMA-511 AC 4 says: "Capability-gated screens (audit, dead-letters) appear only when IAM reports the
matching capability." SMA-511 delivered the audit half. The dead-letters half needs a capability
key, and no such key exists:

- The registry, `enum Capability` in `contracts/proto/paigasus/common/v1/service_info.proto`, holds
  `iam.authz.cedar`, `iam.apikeys`, `iam.audit` and `gateway.chat.stream` only.
- IAM emits only its three keys (`rs/crates/services/paigasus-iam/src/service_info.rs:27-63`).
- `OutboxService` (`contracts/proto/paigasus/iam/v1/iam.proto:602-687`) has no capability.

### 1.1 Acceptance criteria (from the issue)

1. The dead-letters screen appears only when IAM reports the new capability.
2. A console that meets an older IAM without the key shows no dead-letters entry.

---

## 2. Decisions (Sven, 2026-09-19)

| # | Decision | Choice |
|---|---|---|
| D1 | Screen scope | List, replay one entry, discard one entry. **Bulk replay is out of scope** (a follow-up issue, § 10). |
| D2 | Where the key comes from | IAM **always** emits the key. There is no config flag. |
| D3 | Key name | `iam.deadletters` — enum value `CAPABILITY_IAM_DEADLETTERS = 5`. |
| D4 | Page shape | One list page. Each row shows its payload and last error inline. There is no detail page. |

### 2.1 Why D2

`OutboxService` is registered unconditionally, on gRPC and on HTTP. The module comment in
`rs/crates/services/paigasus-iam/src/adapters/grpc/dead_letters.rs:13-18` gives the reason: a
break-glass surface must not be disable-able, because the moment you need it is the moment a config
flag is hardest to change. A config flag that hid only the capability would leave the RPCs on and
would add a toggle with a cosmetic effect only.

So the key means "this IAM build serves `OutboxService`". AC 2 still holds: an IAM build from before
this change does not report the key, and the console hides the screen.

This is the first IAM capability that no config value controls. `Capabilities` today is a set of
config-backed booleans. § 4.2 states how the new key fits that type.

### 2.2 Why D3

The wire-key grammar is `^[a-z][a-z0-9]*(\.[a-z0-9]+)*$` (`service_info.proto:41-53`), so a hyphen
is not possible. The mechanical mapping turns `CAPABILITY_IAM_DEAD_LETTERS` into `iam.dead.letters`,
which reads as a hierarchy that does not exist. `CAPABILITY_IAM_DEADLETTERS` maps to
`iam.deadletters`. The URL segment stays `/iam/dead-letters`; the URL and the key are separate
vocabularies.

### 2.3 Why D4

IAM has no `GetDeadLetter` RPC. A detail page would need a new RPC or a scan of the list. The list
entry already carries every field (`DeadLetterEntry`, `iam.proto:613-625`), so the row shows them.

---

## 3. Scope

### 3.1 In scope

- The proto registry value, the Rust `Capability` tripwire, IAM emission, and regenerated bindings.
- The `@paigasus/discovery` vocabulary.
- `@paigasus/console-core`: the `outbox` client, three `IAM_ACTIONS` entries, and the fake IAM.
- `iam-console`: the nav entry, the `/iam/dead-letters` page, two Server Actions, and tests.

### 3.2 Out of scope

- Bulk replay (`BulkReplayDeadLetters`) in the console.
- The `parked_from` / `parked_to` filter in the console.
- Any change to how IAM authorizes, stores or replays dead letters.
- The gateway console. The gateway has no dead-letter surface.

---

## 4. Contract and IAM

### 4.1 The registry

In `contracts/proto/paigasus/common/v1/service_info.proto`, append after `CAPABILITY_GATEWAY_CHAT_STREAM = 4`:

```proto
// "iam.deadletters" — the Root-only dead-letter queue (OutboxService) is served. IAM always
// reports it: OutboxService has no config switch (a break-glass surface).
CAPABILITY_IAM_DEADLETTERS = 5;
```

The registry is append-only (`service_info.proto:59`). `buf format -w` runs on the file, or
`contracts:fmt` reds without output (memory: proto buf-format).

`contracts:generate` regenerates the Rust, TS and Python bindings. The generated files are
committed. The codegen-drift step in `ci.yml` checks them.

### 4.2 Rust

- `rs/crates/libs/paigasus-proto/src/capability.rs`: `ALL` grows to `[Capability; 5]`. The test
  `adding_a_capability_forces_updating_these_tests` moves its "not registered" probe from `5` to
  `6`. The wire-key round-trip test covers the new value through `ALL`.
- `rs/crates/services/paigasus-iam/src/service_info.rs`: `Capabilities::enabled()` always pushes
  `Capability::IamDeadletters`. `Capabilities` keeps its three config-backed booleans and gets **no**
  fourth field: a field that is always `true` would be a false degree of freedom. The doc comment
  on `enabled()` says that the dead-letters key is unconditional and cites
  `adapters/grpc/dead_letters.rs:13-18`.
- Tests that assert the exact emitted set add `iam.deadletters` to every expected set:
  - `service_info.rs` unit tests, including `every_combination_advertises_exactly_its_enabled_keys`.
    With all three flags `false`, the set is `{"iam.deadletters"}`, not empty.
  - `tests/grpc_service_info.rs` and `tests/http_service_info.rs`.
  - A new unit test asserts that `iam.deadletters` is present for every flag combination.
- The name of the prost variant (`IamDeadletters`) is the generated name. The plan confirms it
  after generation.

### 4.3 Python

Nothing changes by hand. The generated module changes with `contracts:generate`.
`py/packages/paigasus-proto/tests/test_service_info_smoke.py` uses `iam.audit` only.

---

## 5. TS packages

### 5.1 `@paigasus/discovery`

- `ts/packages/paigasus-discovery/src/types.ts:68`: add `'iam.deadletters'` to `CapabilityKey`.
- `ts/packages/paigasus-discovery/tests/vocabulary.test.ts:40-48`: add it to `declared`. This test
  derives the expected list from the generated enum, so it reds until both edits exist.
- `CAPABILITY_KEYS` (`src/core/state.ts:10-12`) derives from the generated schema and needs no edit.

### 5.2 `@paigasus/console-core`

- `src/iam-clients.ts`: `IamClients` gets `outbox`, built with `OutboxService` from
  `@paigasus/sdk`, in the same way as `audit` (lines 20 and 55). `iamClientsForToken` and
  `iamClientsForAction` return it with the others.
- `src/authorize.ts:37-59`: `IAM_ACTIONS` gets `ListOutboxDeadLetters`, `ReplayOutboxDeadLetter` and
  `DiscardOutboxDeadLetter`. The wire names are in `paigasus-iam-core/src/authz/action.rs:151-153`.
  Only `ListOutboxDeadLetters` has a caller in this issue (§ 6.1). The other two are added now so
  that the closed list matches the service the console talks to.
- `testing/fake-iam.ts`: register `outbox: OutboxService` in `SERVICES`, with scripted handlers for
  `listDeadLetters`, `replayDeadLetter` and `discardDeadLetter`, and call counts, in the same way as
  `audit`. `bulkReplayDeadLetters` returns `Unimplemented`, so a call to it in a test is visible.
- The default descriptors (`testing/fake-iam.ts:142`, `testing/dev-world.ts:43-44`) **do not** get
  the new key. A missing key is then the default in every test, so AC 2 needs no special setup. A
  test that needs the screen sets the key.

  Consequence for local development: the dev world does not show the Dead letters entry unless the
  developer adds the key. This is intended. It matches an older IAM.

---

## 6. iam-console

### 6.1 Navigation

- `lib/nav.ts`: `buildNavEntries` gets `deadLettersAllowed: boolean`. When it is true, it pushes
  `{ zone: 'iam', href: '/iam/dead-letters', label: 'Dead letters', state: navStateOf(input.iam, 'iam.deadletters') }`
  directly after the Audit entry.
- `app/(console)/layout.tsx`: `deadLettersAllowed: await may('ListOutboxDeadLetters', ROOT_PRN)`,
  next to the audit call. The two `may(...)` calls run in parallel.
- `mayI()` fails open (SMA-511 § 12). So during an IAM authz outage, a non-Root user can see the
  entry, and IAM refuses the page with a 403. This is the same behavior as Audit.

### 6.2 The gate

`app/(console)/dead-letters/load.ts` exports `deadLettersGate(state)`. It is the audit mapping,
`{ hidden: 'not-found', shown: 'available', degraded: 'degraded' }`, applied to
`capabilityOutcome(state, 'iam.deadletters')`.

The page (`page.tsx`):

1. Reads `searchParams` and the session token, then calls `discovery().getServiceState('iam', token)`.
2. `not-found` → `notFound()`. This covers an absent IAM, and an available IAM without the key.
3. `degraded` → the degraded `ErrorState` with `data-testid="dead-letters-degraded"`, HTTP 200, and
   no IAM call.
4. `available` → validates the query (§ 6.3), builds the clients, and calls the loader.

The page does not call `mayI()` (SMA-511 § 6.3): a typed URL is a user action, and IAM answers it. A
non-Root user gets IAM's `PermissionDenied`, which `callIam` maps to `forbidden` and the page
renders as the 403 view.

### 6.3 The list

- **Query parameters.**
  - `cursor` goes through `parseCursor` (`lib/paging.ts`), as on the audit page.
  - `eventType` is optional. The page trims it and refuses a value longer than 200 characters with
    the `invalid-input` view. An empty value means "no filter". IAM matches `event_type` exactly.
  - Both checks run before the clients are built. A refused query never becomes an IAM call.
- **Loader.** `loadDeadLettersPage({ outbox }, { cursor, eventType })` calls
  `callIam(() => outbox.listDeadLetters({ eventType, cursor, limit: PAGE_SIZE }))`. It maps each
  `DeadLetterEntry` to a plain `DeadLetterRow`:
  `id`, `parkedAt` (ISO or null), `occurredAt` (ISO or null), `eventType`, `schemaVersion`,
  `aggregatePrn`, `actorPrn`, `attempts`, `correlationId`, `payload`, `lastError`.
  The timestamp conversion is the audit page's `occurredAtIso`, moved to `lib/time.ts` so that both
  pages use one copy.
- **Filter form.** A plain GET form with one text input `eventType` and a submit button. It needs no
  JavaScript.
- **Paging.** "First page" and "Next" links, as on the audit page. "Next" keeps `eventType`.
- **Columns.** Parked, Event type, Aggregate, Attempts, Correlation id, and an Actions column.
- **Row details.** Each row has a `<details>` element with the event id, the occurred time, the
  schema version, the actor, the payload and the last error. The payload and the last error render as
  text in a `<pre>`. React escapes them. The page never parses the payload and never renders it as
  HTML.

### 6.4 Replay and discard

- **Commands** (`commands.ts`, `import 'server-only'`): `replayDeadLetter({ outbox }, { id })` and
  `discardDeadLetter({ outbox }, { id })`. Each validates nothing itself; the form schema does.
  `deadLetterForm = z.object({ id: z.string().trim().uuid() })`. They return
  `toActionResult(await callIam(...))`. They take no `mayI` (SMA-511 § 6.3).
- **Actions** (`actions.ts`, `'use server'`): `replayDeadLetterAction` and
  `discardDeadLetterAction`. Each follows the current rules: `iamClientsForAction()` first, zod
  parse through `formFields(form, ['id'])`, `invalidFormInput()` on a parse failure, then the
  command. The existing `tests/unit/actions-structure.test.ts` covers the new file.
- **Revalidation.** After `ok`, and after a `not-found` failure, the action calls
  `revalidatePath('/dead-letters', 'page')`. A `not-found` means that another operator already
  replayed or discarded the entry (`application/dead_letters.rs:129`, `:213`), so the row is stale
  and must go. A plain `forbidden` or `degraded` result does not revalidate. The existing
  `tests/unit/actions-revalidate.test.ts` gets rows for both actions.
- **Where the result shows.** React 19 resets a form before every action, and a revalidate that
  removes the row unmounts every control inside it (memory: action result lost on remount). So:
  - `app/(console)/dead-letters/dead-letter-table.tsx` is a client component. It receives the plain
    rows and the two actions. It owns the result state, and a result region
    (`role="status"`, `data-testid="dead-letters-result"`) above the table.
  - The table component also renders the empty state. It stays mounted when the last row goes away,
    so the result of that last action stays visible.
  - The result text says "Replayed event `<id>`." or "Discarded event `<id>`.", or it shows the
    `PaigasusError` through the existing `FormError`.
  - The row buttons call the actions through a small wrapper that stores the result in the table
    state. The implementation plan picks the exact wiring (one `useActionState` per action at table
    level, with the id in the form data, is the default).
- **Discard confirms.** Discard destroys the entry. It uses the two-step pattern of `ArchiveButton`
  (`app/_components/lifecycle-button.tsx`): the first click shows a confirmation text and a
  "Confirm discard" submit button with a Cancel button. The text is:
  "Discard event `<id>`? IAM deletes it from the dead-letter queue. It is not published again."
  Replay has no confirmation: replay is the normal recovery path, and a second replay of the same id
  returns `not-found`.
- **Pending state.** While an action runs, the row's buttons are disabled.

### 6.5 Error copy

The existing `presentation` table (SMA-511 § 6.5) covers every answer. `not-found` on replay or
discard shows: "This entry is no longer in the dead-letter queue. Another operator replayed or
discarded it." This is a new line in `app/_components/error-copy.ts`, keyed to the two dead-letter
actions, not to `not-found` in general.

---

## 7. Testing

### 7.1 Rust

The tests in § 4.2. `cargo nextest run -p paigasus-proto -p paigasus-iam` covers them. The IAM
integration suites need Docker (CLAUDE.md, Docker-backed suites).

### 7.2 TS unit (vitest)

- `tests/unit/dead-letters-gate.test.ts`: every branch of `deadLettersGate` — absent, available with
  the key, available without the key, degraded.
- `tests/unit/nav.test.ts`: the matrix gets `deadLettersAllowed` × key present/absent × IAM
  up/degraded/absent. The entry order is Organizations, Audit, Dead letters, Gateway.
- `tests/unit/dead-letter-table.test.tsx`: the discard confirmation (Cancel hides it; Confirm
  submits), the result region after a success and after a failure, and the empty state rendered by
  the same component.
- `tests/unit/actions-revalidate.test.ts`: rows for both actions: `ok` and `not-found` revalidate,
  `forbidden` does not.
- `tests/unit/lib-time.test.ts` (or the existing audit test, moved): the out-of-range timestamp
  returns null.

### 7.3 TS integration (vitest, fake IAM)

- `tests/integration/dead-letters-page.test.ts`: the loader maps every field; `eventType` and the
  cursor reach IAM; an empty `nextCursor` becomes `null`; an IAM error becomes the `PaigasusError`.
- `tests/integration/dead-letter-commands.test.ts`: replay and discard send the id; an invalid UUID
  never reaches IAM; `NotFound` maps to `not-found`.

### 7.4 e2e (Playwright, `iam-console-ts:test-e2e`)

New rows, in `tests/e2e/dead-letters.spec.ts`:

| Row | Scenario | Asserts |
|---|---|---|
| R17 | IAM reports `iam.deadletters`; the user is Root | The nav shows "Dead letters". The page lists the fake IAM's entries. Replay removes the row and shows the result text; `ReplayDeadLetter` was called once with the id. Discard needs the confirmation, then removes the row; `DiscardDeadLetter` was called once. |
| R18 | IAM does not report `iam.deadletters` (AC 2) | No "Dead letters" nav entry. `GET /iam/dead-letters` returns 404. `ListDeadLetters` was never called. |
| R19 | IAM is degraded | The entry is disabled with a reason. `GET /iam/dead-letters` returns 200 with `dead-letters-degraded`. `ListDeadLetters` was never called. |

- `tests/e2e/support/world.ts`: `ALL_ACTIONS` gets the three new actions. `DEFAULT_DESCRIPTOR` keeps
  its two keys (§ 5.2). R17 sets the key with `harness.useWorld({ descriptor: { capabilities: [...] } })`.
- `tests/unit/e2e-rows.test.ts`: the row count goes from 16 to 19, and its header comment names this
  spec.

### 7.5 Gates to run before the push

The full CI target list from CLAUDE.md. The proto edit selects `contracts:*`, every binding, and the
codegen-drift step. `:breaking` must stay green: an appended enum value is not a breaking change.

---

## 8. Compatibility

- **New console, old IAM.** No key → no entry, and a typed URL is a 404. This is AC 2.
- **Old console, new IAM.** The old console's `CAPABILITY_KEYS` does not know `iam.deadletters`.
  The discovery client ignores an unknown key (`service_info.proto:70-82`, "unknown = ignored").
  Nothing changes for that console.
- **Other consumers of `ServiceInfo`.** The Rust, Python and TS clients decode `capabilities` as
  `repeated string`. A new string is additive.

---

## 9. Risks and recorded limits

- **The key does not prove that the caller may use the screen.** It says that the service exists.
  `mayI()` hides the nav entry, and IAM refuses a non-Root caller. This is the same as Audit.
- **Replay can publish a duplicate** when the broker's dedup window has passed
  (`application/dead_letters.rs:29-55`). The console does not warn about this. IAM already records
  the exposure as a metric label. A warning in the UI is a possible follow-up, not in this issue.
- **The payload may hold personal data.** The screen shows it to Root users only, which IAM already
  allows through the API. The console does not log it.

---

## 10. Follow-ups

A new Linear issue: bulk replay (`BulkReplayDeadLetters`) and the parked-time filter in
`/iam/dead-letters`. The bulk form must require `max_rows` (0 is invalid, `iam.proto:665-677`).
