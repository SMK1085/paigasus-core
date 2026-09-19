# SMA-629 — the `iam.deadletters` capability key and the `/iam/dead-letters` screen

- **Issue:** SMA-629 (split from SMA-511, decision D1).
- **Status:** revision 2. Sven approved the design in chat on 2026-09-19. Revision 2 adds the
  findings of the Stage 2 challenge (§ 12).
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
2. A console that meets an older IAM without the key shows no dead-letters entry. (One exception,
   the same as for Audit: while IAM is degraded, see § 8.)

---

## 2. Decisions (Sven, 2026-09-19)

| # | Decision | Choice |
|---|---|---|
| D1 | Screen scope | List, replay one entry, discard one entry. **Bulk replay is out of scope** (a follow-up issue, § 11). |
| D2 | Where the key comes from | IAM **always** emits the key. There is no config flag. |
| D3 | Key name | `iam.deadletters` — enum value `CAPABILITY_IAM_DEADLETTERS = 5`. |
| D4 | Page shape | One list page. Each row shows its payload and last error inline. There is no detail page. |

### 2.1 Why D2

`OutboxService` is registered unconditionally, on gRPC (`adapters/grpc/mod.rs:96-101`) and on HTTP
(`adapters/http/mod.rs:921`, `:1044`). The module comment in
`rs/crates/services/paigasus-iam/src/adapters/grpc/dead_letters.rs:13-18` gives the reason: a
break-glass surface must not be disable-able, because the moment you need it is the moment a config
flag is hardest to change. A config flag that hid only the capability would leave the RPCs on and
would add a toggle with a cosmetic effect only.

So the key means "this IAM build serves `OutboxService`". AC 2 still holds: an IAM build from before
this change does not report the key, and the console hides the screen.

This is the first IAM capability that no config value controls. § 4.2 states how the new key fits
the `Capabilities` type. § 11 records a note for ADR-0020.

### 2.2 Why D3

The wire-key grammar is `^[a-z][a-z0-9]*(\.[a-z0-9]+)*$` (`service_info.proto:41-53`), so a hyphen
is not possible. The mechanical mapping turns `CAPABILITY_IAM_DEAD_LETTERS` into `iam.dead.letters`,
which reads as a hierarchy that does not exist. `CAPABILITY_IAM_DEADLETTERS` maps to
`iam.deadletters`. The URL segment stays `/iam/dead-letters`. The URL and the key are separate
vocabularies.

### 2.3 Why D4

IAM has no `GetDeadLetter` RPC. A detail page would need a new RPC or a scan of the list. The list
entry already carries every field (`DeadLetterEntry`, `iam.proto:613-625`), so the row shows them.

---

## 3. Scope

### 3.1 In scope

- The proto registry value, the Rust and TS spelling tests, IAM emission, and regenerated bindings.
- The `@paigasus/discovery` vocabulary.
- `@paigasus/console-core`: the `outbox` client, one `IAM_ACTIONS` entry, the test fake, and the
  dev world.
- `iam-console`: the nav entry, the `/iam/dead-letters` page, two Server Actions, and tests.
- One line in `docs/ops/RUNBOOK-observability.md` that names the screen.

### 3.2 Out of scope

- Bulk replay (`BulkReplayDeadLetters`) in the console.
- The `parked_from` / `parked_to` filter in the console.
- A warning on Replay when the entry is older than the broker's dedup window (§ 9).
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

The registry is append-only (`service_info.proto:59`). Run `buf format -w` on the file:
`contracts:fmt` fails on an unformatted `.proto`, and its failure output is easy to miss in a
`moon ci` run.

`contracts:generate` regenerates the Rust, TS and Python bindings. The generated files are
committed. The codegen-drift step in `ci.yml` checks them. `:breaking` stays green, because an
appended enum value is not a breaking change.

### 4.2 Rust

- `rs/crates/libs/paigasus-proto/src/capability.rs`:
  - `ALL` grows to `[Capability; 5]`.
  - `adding_a_capability_forces_updating_these_tests` moves its "not registered" probe from `5` to `6`.
  - `the_registry_spells_the_adr_keys_exactly` (`:80-85`) gets `iam.deadletters`. This test pins
    D3's spelling. A round-trip test alone would also pass for `iam.dead.letters`.
- `rs/crates/services/paigasus-iam/src/service_info.rs`:
  - `Capabilities::enabled()` always pushes the new variant. The plan confirms the prost name
    (`IamDeadletters` is expected) after generation.
  - `Capabilities` keeps its three config-backed booleans and gets **no** fourth field. A field that
    is always `true` would be a false degree of freedom.
  - The doc comment on `enabled()` says that the dead-letters key is unconditional and cites
    `adapters/grpc/dead_letters.rs:13-18`.
- The unit tests in `service_info.rs` change as follows:

  | Test | Edit |
  |---|---|
  | `all_enabled_advertises_every_iam_capability` (`:93`) | The expected set gets the new variant. |
  | `all_disabled_advertises_nothing` (`:96-99`) | Rename to `all_flags_off_advertises_only_iam_deadletters`. Assert the set is exactly `{IamDeadletters}`. |
  | `disabling_one_flag_removes_exactly_its_key` (`:106-108`) | Each expected set gets the new variant. |
  | `every_combination_advertises_exactly_its_enabled_keys` (`:114-125`) | It checks `contains` per flag and does not fail. Add an assertion that the new key is present in every combination. |

- The integration tests change as follows:
  - `tests/grpc_service_info.rs` (`:108`, `:153-201`): every expected set gets `iam.deadletters`.
  - `tests/http_service_info.rs` (`:52-156`): the same.
  - `tests/http_service_info.rs:159-179`, `all_capabilities_disabled_serves_an_empty_array_not_a_missing_field`:
    IAM can no longer emit an empty list. Rename the test to
    `all_capability_flags_off_serves_only_iam_deadletters`, and assert `json!(["iam.deadletters"])`.
    Keep its second purpose: conditional router merging must not panic with every flag off. The
    empty-array rule itself stays proven by `rs/crates/services/paigasus-gateway/tests/service_info.rs:281-295`
    and `rs/crates/libs/paigasus-service-info/src/lib.rs:107`. The test's doc comment says this.

### 4.3 Python

Nothing changes by hand except one test. The generated module changes with `contracts:generate`.
`py/packages/paigasus-proto/tests/test_service_info_smoke.py:17-22` lists the enum names. Add the
fifth name for symmetry. It does not fail without the edit.

---

## 5. TS packages

### 5.1 `@paigasus/proto`

`ts/packages/paigasus-proto/src/capability.test.ts`:
- `:28` expects 5 members. The enum now has 6, with `UNSPECIFIED`.
- The literals test (`:7-12`) gets `iam.deadletters`. It is the TS test that pins D3's spelling.

### 5.2 `@paigasus/discovery`

- `ts/packages/paigasus-discovery/src/types.ts:68`: add `'iam.deadletters'` to `CapabilityKey`.
- `ts/packages/paigasus-discovery/tests/vocabulary.test.ts:40-48`: add it to `declared`. This test
  derives the expected list from the generated enum, so it fails until both edits exist.
- `CAPABILITY_KEYS` (`src/core/state.ts:10-12`) derives from the generated schema and needs no edit.

### 5.3 `@paigasus/console-core`

- `src/iam-clients.ts`: `IamClients` gets `outbox`, built with `OutboxService` from
  `@paigasus/sdk`, in the same way as `audit` (lines 20 and 55). `iamClientsForToken` and
  `iamClientsForAction` return it with the others. A new integration test,
  `tests/integration/outbox-client.test.ts`, proves that the client reaches `OutboxService`. It
  follows `tests/integration/service-accounts-client.test.ts`.
- `src/authorize.ts:37-59`: `IAM_ACTIONS` gets `ListOutboxDeadLetters` only. The layout asks about
  it (§ 6.1). Replay and discard have no `mayI` caller: the page is Root-only, and a user who can
  list can act. The existing `tests/unit/action-names.test.ts` checks the new name against IAM's
  Rust action catalog.
- `testing/fake-iam.ts`: register `outbox: OutboxService` in `SERVICES`. Add no default handlers:
  `audit` has none either, and every unscripted method already throws `Unimplemented`
  (`:248-260`). Update the header comment, which says "six services" (`:4`).
- The fake's `DEFAULT_DESCRIPTOR` (`testing/fake-iam.ts:142`) **does not** get the new key. A
  missing key is then the default in every test, so AC 2 needs no special setup. A test that needs
  the screen sets the key.
- `testing/dev-world.ts` models a **current** IAM, because it serves a person who clicks through the
  app (`:9-11`). So:
  - `DEV_IAM_DESCRIPTOR` (`:43-44`) gets `iam.deadletters`.
  - The dev world gets stateful `listDeadLetters`, `replayDeadLetter` and `discardDeadLetter`
    handlers over three seeded entries. Replay and discard remove the entry. An unknown id answers
    `NotFound`. The ids are RFC 4122 UUIDs.
  - `tests/unit/dev-world.test.ts`: the descriptor assertion (`:83`) and `REQUIRED` (`:11-37`) get
    the new key and the three methods.

---

## 6. iam-console

### 6.1 Navigation

- `lib/nav.ts`: `buildNavEntries` gets `deadLettersAllowed: boolean`. When it is true, it pushes
  `{ zone: 'iam', href: '/iam/dead-letters', label: 'Dead letters', state: navStateOf(input.iam, 'iam.deadletters') }`
  directly after the Audit entry.
- `app/(console)/layout.tsx`: `deadLettersAllowed: await may('ListOutboxDeadLetters', ROOT_PRN)`.
  The two `may(...)` calls run in parallel. The header comment (`:4`, "five request-scoped reads")
  is updated. Every console render now makes one more `IsAuthorized` call to IAM.
- `mayI()` fails open (SMA-511 § 12). So during an IAM authz outage, a non-Root user can see the
  entry, and IAM refuses the page with a 403. This is the same as for Audit.

### 6.2 The gate

`app/(console)/dead-letters/load.ts` exports `deadLettersGate(state)`. It is the audit mapping,
`{ hidden: 'not-found', shown: 'available', degraded: 'degraded' }`, applied to
`capabilityOutcome(state, 'iam.deadletters')`.

The page (`page.tsx`):

1. Reads `searchParams` and the session token, then calls `discovery().getServiceState('iam', token)`.
2. `not-found` → `notFound()`. This covers an absent IAM, and an available IAM without the key.
3. `degraded` → the degraded view (§ 6.4), HTTP 200, and no IAM call.
4. `available` → validates the query (§ 6.3), builds the clients, and calls the loader.

The page does not call `mayI()` (SMA-511 § 6.3): a typed URL is a user action, and IAM answers it.

### 6.3 The query and the list

- **`lib/paging.ts`** exports two new functions, next to `parseCursor`, with the same result shape
  (`{ ok: true, value } | { ok: false, error: PaigasusError }`):
  - `parseEventType(raw)` reads the first value when `raw` is an array (it uses the private
    `first()`), trims it, and returns `''` for no filter. It refuses a value longer than 200
    characters with an `invalid-input` `PaigasusError`. 200 is the console's existing bound for a
    short identifier (`lib/form.ts:13`). IAM matches `event_type` exactly.
  - `listHref(path, query)` builds an href from a map and **leaves out every empty value**. It is
    the gateway console's `linkHref` (`ts/apps/gateway-console/lib/paging.ts:40`), copied. The
    private `cursorTooLong()` gets a sibling `eventTypeTooLong()`.
- Both parsers run before the clients are built. A refused query never becomes an IAM call. A
  refused query renders `PageError` with the `invalid-input` error, as a refused cursor does today.
- **Loader.** `loadDeadLettersPage({ outbox }, { cursor, eventType })` calls
  `callIam(() => outbox.listDeadLetters({ eventType, cursor, limit: PAGE_SIZE }))`. It maps each
  `DeadLetterEntry` to a plain `DeadLetterRow`:

  | Field | Mapping |
  |---|---|
  | `id`, `eventType`, `aggregatePrn`, `payload` | as sent |
  | `schemaVersion`, `attempts` | as sent (numbers) |
  | `parkedAt`, `occurredAt` | ISO string, or `null` for a missing or out-of-range timestamp |
  | `actorPrn`, `correlationId`, `lastError` | the proto uses "empty means none" (`iam.proto:619,621,624`), so `''` maps to `null`. The UI shows "—" for `null`. |

  The timestamp conversion is the audit page's `occurredAtIso`, moved to `lib/time.ts` so that both
  pages use one copy.
- **Order.** IAM orders the list by `id` descending (`tests/dead_letters_pg.rs:432`). IAM mints
  UUIDv7 ids, so this is close to creation order, not park order. The page keeps IAM's order and
  does not sort. The first column is "Event id", and the table caption says "Newest events first".
- **Filter form.** A plain GET form with one text input, `eventType`, and a submit button. It has
  **no `action` attribute**, so it submits to the current URL, `/iam/dead-letters`. A basePath-relative
  `action="/dead-letters"` would leave the zone. The form needs no JavaScript.
- **Paging.** "First page" and "Next" links, as on the audit page. Both keep `eventType`. Both use
  `listHref('/iam/dead-letters', …)`, so an empty value is not in the URL.
- **Columns.** Event id, Parked, Event type, Aggregate, Attempts, Correlation id, Actions.
- **Row details.** Each row has a `<details>` element with the occurred time, the schema version, the
  actor, the payload and the last error. The payload and the last error render as text in a `<pre>`
  with `whitespace-pre-wrap break-all` and a maximum height with scroll. The payload has no size
  bound (only `last_error` has one, 1 KB, `adapters/events/relay.rs:56-59`). React escapes the
  text. The page never parses the payload and never renders it as HTML.

### 6.4 The frame, and replay and discard

**Why a frame.** React 19 resets a form before every action. A successful replay or discard removes
the row, and a revalidated render can also replace the table with an error view. A control that
holds its own result loses it when it unmounts. The gateway console solved this in SMA-636 with a
client frame that holds the only result region
(`ts/apps/gateway-console/app/_components/service-account-frame.tsx:1-27`). This page uses the same
design.

- **`app/(console)/dead-letters/dead-letters-frame.tsx`** is a client component. It holds:
  - one result region (`role="status"`, `data-testid="dead-letters-result"`) above its children;
  - one runner for both actions.
- **Where the page renders it.** After the gate, the page renders the frame for **every** view that
  does not throw: the table, the empty list, the degraded view, and every list error except two.
  A list error with `presentation` `forbidden` or `not-found` still goes to `PageError`, so a fresh
  GET keeps its real 403 or 404. Every other list error renders inside the frame as a
  `SectionError`. A revalidated render therefore keeps the frame and its result.
- **Frame key.** The page renders the frame with `key={`${eventType}|${cursor}`}`. A move to another
  page or filter starts a new, empty frame, so no result from the previous page remains.
- **The runner.** It follows the gateway frame:
  - A row form hands its `FormData` to the runner. The runner calls the Server Action **directly**
    in a transition, not through `useActionState`.
  - It records the id from the `FormData`, so the result text can name it.
  - A generation counter makes sure that the newest submission's result wins.
  - It catches a rejected promise (a network drop or a server fault) and shows a client-built error.
    It never rethrows: a rethrow reaches `(console)/error.tsx`, which unmounts the frame. The text
    says that the result is unknown and that the operator must reload the page to see the current
    queue.
  - While an action runs, **every** Replay and Discard button in the frame is disabled.
- **Result text.** "Replayed event `<id>`." or "Discarded event `<id>`." For a failure, the region
  shows `FormError` (§ 6.5).
- **Discard confirms.** Discard deletes the entry. It uses the two-step pattern of `ArchiveButton`
  (`app/_components/lifecycle-button.tsx:42-80`): the first click shows a confirmation text and a
  "Confirm discard" submit button with a Cancel button. The text is: "Discard event `<id>`? IAM
  deletes it from the dead-letter queue, and it is not published again. The audit log keeps a copy
  of the event." (`application/dead_letters.rs:215-229`.)
- **Replay has no confirmation.** Replay is the normal recovery path. A replay of an id that is no
  longer parked returns `not-found`. (The relay can park the same row again later, if the new
  publish also fails, `pg_dead_letters.rs:45-46`.)
- **Commands** (`commands.ts`, `import 'server-only'`): `replayDeadLetter({ outbox }, { id })` and
  `discardDeadLetter({ outbox }, { id })`. They return `toActionResult(await callIam(...))`. They
  take no `mayI` (SMA-511 § 6.3).
- **Form schema.** `deadLetterForm = z.object({ id: z.uuid() })` after a trim. zod 4's `z.uuid()`
  is RFC-strict and refuses some ids that IAM's `Uuid::parse_str` accepts. This is acceptable: IAM
  mints UUIDv7 ids, which are RFC 4122 ids.
- **Actions** (`actions.ts`, `'use server'`): `replayDeadLetterAction` and `discardDeadLetterAction`.
  Each follows the current rules: `iamClientsForAction()` first, a zod parse through
  `formFields(form, ['id'])`, `invalidFormInput()` on a parse failure, then the command.
- **Revalidation.** A pure helper, `refreshesAfterDeadLetterAction(result)` in `lib/form.ts`, next to
  `refreshesAfterLifecycleAction`, returns `true` for `ok` and for a `not-found` failure. The action
  then calls `revalidatePath('/dead-letters', 'page')`.
  - `not-found` means that the entry is no longer parked, so the row is stale and must go.
  - `forbidden` does **not** revalidate, unlike the lifecycle rule. A forbidden replay means that the
    caller is not Root, so the list read is forbidden too. The re-render would call `forbidden()`
    and replace the whole page with the 403 view, and the inline error would be lost.
  - Other failures change nothing on the page and do not revalidate.

### 6.5 Error copy

- `app/_components/form-error.tsx` gets an optional `message` prop, as in the gateway console
  (`ts/apps/gateway-console/app/_components/form-error.tsx:18,24`). The prop replaces
  `formMessage(error)` only when it is set.
- `app/_components/error-copy.ts` exports `DEAD_LETTER_GONE`: "This entry is no longer in the
  dead-letter queue. It was already replayed or discarded, maybe by an earlier attempt of this
  request." (`docs/ops/RUNBOOK-observability.md:447-455`.)
- The frame passes `DEAD_LETTER_GONE` only for a `not-found` result of the two dead-letter actions.
  The global `FORM_REASON_COPY[ErrorReason.NOT_FOUND]` (`error-copy.ts:39`) does not change.
- Every other answer uses the existing copy (SMA-511 § 6.5).

---

## 7. Testing

### 7.1 Rust

The tests in § 4.2. `cargo nextest run -p paigasus-proto -p paigasus-iam` covers them. The IAM
integration suites need Docker.

### 7.2 TS unit (vitest)

- `tests/unit/dead-letters-gate.test.ts`: every branch of `deadLettersGate` — absent, available with
  the key, available without the key, degraded, and degraded with a cached descriptor that has no
  key (→ degraded, § 8).
- `tests/unit/nav.test.ts`: the matrix gets `deadLettersAllowed` × key present/absent × IAM
  up/degraded/absent. The entry order is Organizations, Audit, Dead letters, Gateway.
- `tests/unit/paging.test.ts`: `parseEventType` (array input, trim, empty, 200 and 201 characters)
  and `listHref` (empty values left out, encoding).
- `tests/unit/form.test.ts`: `refreshesAfterDeadLetterAction` for `ok`, `not-found`, `forbidden`,
  `degraded`.
- `tests/unit/dead-letters-frame.test.tsx` (jsdom):
  - Rows [A, B]. Replay A. Rerender with [B], then with [], then with a `SectionError` child. The
    result text "Replayed event A." stays after each step.
  - A rejected action shows the client-built error and does not throw.
  - Two submissions: the newer result wins.
  - Every button is disabled while an action runs.
  - Discard: Cancel hides the confirmation; Confirm submits.
  - `not-found` shows `DEAD_LETTER_GONE`.
  - A payload that holds `</pre><script>alert(1)</script>` renders as text.
- `tests/unit/lib-time.test.ts`: the out-of-range timestamp returns `null` (moved from the audit
  tests if one exists there).
- Existing tests that change:

  | Test | Edit |
  |---|---|
  | `tests/unit/actions-structure.test.ts:25-30` | `EXPECTED` gets `'(console)/dead-letters/actions.ts': ['discardDeadLetterAction', 'replayDeadLetterAction']`. |
  | `tests/unit/actions-revalidate.test.ts:51` | The `iamClientsForAction` mock gets `outbox`. A new `describe` expects `{ path: '/dead-letters', type: 'page' }` for `ok` and `not-found`, and no call for `forbidden`. |
  | `tests/unit/action-session.test.ts:110` | The exact `IamClients` key list gets `outbox`. |
  | `tests/unit/e2e-rows.test.ts` | The row count goes from 16 to 19. The header comment names this spec. |

### 7.3 TS integration (vitest, fake IAM)

- `tests/integration/dead-letters-page.test.ts`: the loader maps every field, including `''` to
  `null`; `eventType` and the cursor reach IAM; an empty `nextCursor` becomes `null`; an IAM error
  becomes the `PaigasusError`.
- `tests/integration/dead-letter-commands.test.ts`: replay and discard send the id; `NotFound` maps
  to `not-found`.
- The action test (`actions-revalidate.test.ts`, or a new action test) asserts that an invalid id
  makes **zero** `replayDeadLetter` / `discardDeadLetter` calls. This check belongs at the action
  level, because the command does not validate.

### 7.4 e2e (Playwright, `iam-console-ts:test-e2e`)

New rows, in `tests/e2e/dead-letters.spec.ts`:

| Row | Scenario | Asserts |
|---|---|---|
| R17 | IAM reports `iam.deadletters`; the user is Root | The nav shows "Dead letters". The page lists two entries. Replay removes one row and shows the result text; `ReplayDeadLetter` was called once with its id. Discard needs the confirmation, then removes the other row; `DiscardDeadLetter` was called once. The empty state shows, and the discard result text is still visible. |
| R18 | IAM does not report `iam.deadletters` (AC 2) | No "Dead letters" nav entry. `GET /iam/dead-letters` returns 404. `ListDeadLetters` was never called. |
| R19 | IAM is degraded | The entry is disabled with a reason. `GET /iam/dead-letters` returns 200 with `dead-letters-degraded`. `ListDeadLetters` was never called. |

`tests/e2e/support/world.ts` changes:
- `ALL_ACTIONS` gets `ListOutboxDeadLetters`.
- The default handler set gets `outbox.listDeadLetters`, `outbox.replayDeadLetter` and
  `outbox.discardDeadLetter`, because every override key must exist in the default set (`:3-6`).
  The defaults are stateful: they hold two seeded entries with RFC 4122 UUID ids, and replay and
  discard remove the entry. An unknown id answers `NotFound`.
- `DEFAULT_DESCRIPTOR` keeps its two keys (§ 5.3). R17 overrides the descriptor with the full
  `Descriptor` shape — `service`, `version` and `capabilities` — because the type needs all three
  (`:72`).
- R17 calls `waitForHydration` after every `page.goto` and before the first click, because Discard
  needs JavaScript.

`tests/e2e/token-leak.spec.ts` (R11): the world sets the key, the path list (`:170`) gets
`/iam/dead-letters`, and the test runs one replay. The new page and its two Server Actions are a new
Flight surface, and R11 is the test that checks that no token reaches the browser.

### 7.5 Gates to run before the push

The full CI target list from CLAUDE.md. The proto edit selects `contracts:*`, every binding, and the
codegen-drift step.

---

## 8. Compatibility

- **New console, old IAM.** No key → no entry, and a typed URL is a 404. This is AC 2.
- **New console, old IAM that is degraded.** `capabilityOutcome` answers `degraded` whatever the key
  (`ts/packages/paigasus-discovery/src/core/outcome.ts:32-34`). So the entry shows as disabled with a
  reason, and the page shows the degraded view. This is the same as for Audit. When IAM recovers,
  the entry goes away.
- **Old console, new IAM.** The old console's `CAPABILITY_KEYS` does not know `iam.deadletters`.
  The discovery client ignores an unknown key (`service_info.proto:70-82`). Nothing changes for that
  console.
- **Other consumers of `ServiceInfo`.** The Rust, Python and TS clients decode `capabilities` as
  `repeated string`. A new string is additive.

---

## 9. Risks and recorded limits

- **The key does not prove that the caller may use the screen.** It says that the service exists.
  `mayI()` hides the nav entry, and IAM refuses a non-Root caller. This is the same as for Audit.
- **Replay can publish a duplicate** when the broker's dedup window has passed
  (`application/dead_letters.rs:28-56`). The console does not warn about this. IAM already records
  the exposure as a metric label. A warning is out of scope (§ 3.2).
- **The payload may hold personal data.** The screen shows it to Root users only, which IAM already
  allows through the API. The console does not log it.
- **A lost network response.** When a replay or discard response is lost, the runner cannot know the
  result. It says so and asks the operator to reload. A later retry of the same id answers
  `not-found` with `DEAD_LETTER_GONE`, which covers this case.

---

## 10. Documentation

`docs/ops/RUNBOOK-observability.md:372-416` lists the API and gRPC paths for dead letters. Add one
line: a Root user can also list, replay and discard entries at `/iam/dead-letters` in the IAM
console. Bulk replay stays API-only.

---

## 11. Follow-ups

- A new Linear issue: bulk replay (`BulkReplayDeadLetters`) and the parked-time filter in
  `/iam/dead-letters`. The bulk form must require `max_rows` (0 is invalid, `iam.proto:665-677`).
- A note on ADR-0020 in Notion: a capability key can mean "this build serves the surface", with no
  config switch behind it. `iam.deadletters` is the first such key.

---

## 12. Challenge log (revision 1 → 2)

| Finding | Result | Where |
|---|---|---|
| MAJOR — the result region is lost when a revalidated render swaps the table; one table-wide `pending`; a rejected promise | accepted; a frame as in the gateway console | § 6.4, § 7.2 |
| MAJOR — § 6.4 and § 6.5 disagree on the `not-found` copy; the copy claimed "another operator" | accepted; `message` prop and `DEAD_LETTER_GONE` with corrected text | § 6.5 |
| MAJOR — six existing tests fail and were not listed; `all_disabled_advertises_nothing` and the HTTP empty-array test change meaning | accepted | § 4.2, § 5.1, § 7.2 |
| MINOR — the e2e world: default handlers, stateful removal, UUID fixture ids, hydration, `Descriptor` shape | accepted | § 7.4 |
| MINOR — the dev world modelled an old IAM and had no outbox handlers | accepted; the dev world is a current IAM | § 5.3 |
| MINOR — query handling: arrays, hrefs, empty values, form `action`, named helper, the 200 bound | accepted | § 6.3 |
| MINOR — empty-means-none mapping, list order, payload overflow, escaping test | accepted | § 6.3, § 7.2 |
| MINOR — the invalid-UUID test was at the command level; zod is stricter than IAM | accepted | § 6.4, § 7.3 |
| MINOR — the revalidate rule was prose only; no reason for `forbidden` | accepted; a helper and the reason | § 6.4 |
| MINOR — fake-IAM wording | accepted | § 5.3 |
| MINOR — AC 2 exception while IAM is degraded | accepted | § 1.1, § 8 |
| MINOR — replay re-park, audit copy on discard, layout comment, extra `IsAuthorized` | accepted | § 6.1, § 6.4 |
| MINOR — R11 does not cover the new surface | accepted | § 7.4 |
| MINOR — the spec cited private notes | accepted; the facts are now written out | § 4.1, § 6.4 |
| MINOR — the runbook does not name the screen | accepted | § 10 |
| QUESTION — replay and discard in `IAM_ACTIONS` with no caller | only `ListOutboxDeadLetters` is added; the page is Root-only, so no button needs its own `mayI` | § 5.3 |
| QUESTION — a dedup-window warning on Replay | rejected for this issue: scope; recorded as a risk | § 3.2, § 9 |
| QUESTION — an ADR-0020 note | accepted as a follow-up, not in this PR | § 11 |
| QUESTION — key the frame by filter and cursor | accepted | § 6.4 |
