# SMA-661 — bulk replay and the parked-time filter on `/iam/dead-letters`

- **Issue:** SMA-661, a follow-up from SMA-629 (spec § 11, decision D1).
- **Status:** revision 1. Sven approved the design in chat on 2026-09-21.
- **Parent spec:** `docs/superpowers/specs/2026-09-19-sma-629-dead-letters-capability-design.md`
  (§ 5.3, § 6.3, § 6.4, § 6.5, § 7, § 9, § 11).

---

## 1. Problem

SMA-629 delivered the `/iam/dead-letters` screen: a list, a single replay and a single discard.
Decision D1 of that spec put two things out of scope, and this issue delivers them:

1. **A parked-time filter on the list.** `ListDeadLettersRequest` carries `parked_from` and
   `parked_to` (`contracts/proto/paigasus/iam/v1/iam.proto:666-672`). The console sends neither.
   An operator cannot narrow the list to the incident window.
2. **Bulk replay.** `BulkReplayDeadLetters` exists on `OutboxService` (`iam.proto:717`). No console
   code calls it. An operator must replay a large backlog one row at a time.

Nothing in the backend changes. The proto, the Rust service and the generated TypeScript client all
carry both features already. The work is in `ts/apps/iam-console`, in
`ts/packages/paigasus-console-core` (one action name), and in the e2e world.

### 1.1 Acceptance criteria (from the issue)

1. A bulk replay with no `max_rows` never reaches IAM.
2. The confirmation names the maximum number of events that the action can re-inject.
3. The parked-time filter reaches IAM and survives paging.
4. A unit test covers the filter parser, and an e2e row covers one bulk replay against the fake IAM.

---

## 2. Decisions (Sven, 2026-09-21)

| # | Decision | Choice |
|---|---|---|
| D1 | Where the bulk scope comes from | The bulk form is **bound to the list filter**. It shows the active scope as read-only text and posts it as hidden fields. The operator types only `max_rows`. |
| D2 | How a parked bound is written | A **text input** that needs a full ISO 8601 instant with `Z` or a `±HH:MM` offset. A value with no zone is `invalid-input`. |
| D3 | The 10000 ceiling | The console **refuses** `max_rows` outside `1..10000`. A unit test pins the bound to IAM's Rust constant. |
| D4 | Authorization | The page asks `mayI('ReplayOutboxDeadLetter', ROOT_PRN)`. A `false` answer hides the **bulk form and the row Replay buttons**. Discard is unchanged. |
| D5 | An empty scope | **Allowed.** The row budget is the only guard, as the proto states. The confirmation names the empty scope in words. |

### 2.1 Why D1

The screen's value is that the table is the evidence for what the button does. A bulk form with its
own inputs can name a scope the table never showed, on an action that re-injects up to 10000 events.
Binding the form to the filter removes that gap: the only way to change the bulk scope is to change
the filter and read the list again.

### 2.2 Why D2

A `datetime-local` input carries no zone. The browser shows the operator their local time, and the
server reads the same digits as UTC. The window the confirmation names would then not be the window
the operator saw. A hidden offset field removes the ambiguity but needs JavaScript, and the filter
form is deliberately a plain GET form that works without it (SMA-629 § 6.3).

A text instant also matches the vocabulary already on screen: `load.ts` maps `parked_at` through
`timestampIso` (`lib/time.ts`), so the table prints ISO instants. The operator types what they read.

### 2.3 Why D3

`max_rows` above 10000 is silently clamped by IAM, on both transports, with no signal to the caller
(`iam.proto:703-709`). A confirmation that named a larger number would be false, and AC 2 requires
it to be true. Refusing the value keeps the sentence exact.

The cost is a constant duplicated across languages. § 7.2 pins it with a drift test.

### 2.4 Why D4, and what it does not buy

Sven chose to add the `mayI` question. The recorded counter-argument, which the spec keeps so that a
later reader sees it:

- Every `OutboxService` RPC is Root-only inside IAM. A caller who cannot list gets `forbidden` and
  the page renders `PageError`, so such a caller never sees the frame at all.
- `mayI` is cosmetic and **fails open**: on a failed authorization query, or an unnamed principal,
  it answers `true` and the control shows (`paigasus-console-core/src/authorize.ts`).

So the question hides a control from nobody who could otherwise have used it, and it cannot be
relied on as a guard. IAM stays the only real decision. The value it does add is an affordance that
matches the caller's permissions when the query succeeds.

D4 reaches the row Replay buttons as well, because they invoke the same Cedar action. Hiding the
bulk form alone would put two contradictory rules for one permission on one page.

`DiscardOutboxDeadLetter` stays out of `IAM_ACTIONS`, so a row can show Discard without Replay. That
is truthful: they are separate permissions.

### 2.5 Why D5

The proto states the rule directly: "the explicit row budget is the guard on blast radius"
(`iam.proto:705-707`). Draining the queue with a small, stated budget is a break-glass case this
screen exists for. A "set at least one filter" rule adds a second, weaker guard that an
everything-matching filter passes anyway.

---

## 3. Facts this design rests on

Each is measured in the repository, not assumed.

1. `BulkReplayDeadLettersRequest` holds `event_type`, `parked_from`, `parked_to`, `max_rows`
   (`iam.proto:699-711`). `BulkReplayDeadLettersResponse` holds `replayed` only.
2. `max_rows` is `uint64` and `replayed` is `uint64`. protobuf-es maps both to **`bigint`**
   (`ts/packages/paigasus-proto/src/generated/paigasus/iam/v1/iam_pb.ts`).
3. `max_rows` 0 is rejected before any store access. An absent field collapses to 0, so "did not
   say" and "said zero" are refused identically. This is deliberate (`iam.proto:703-707`).
4. The clamp is `BulkReplayRequest::MAX_BULK_REPLAY = 10_000`
   (`rs/crates/libs/paigasus-iam-core/src/dead_letter.rs:77`). `capped_max_rows()` applies it
   (`:83`). The audit record keeps `max_rows` and `capped_max_rows` as separate fields
   (`rs/crates/services/paigasus-iam/src/application/dead_letters.rs:183-193`).
5. Bulk replay is **not atomic**. A `DEADLINE_EXCEEDED` or a cancelled RPC can leave an unknown
   number of rows already replayed. Re-issuing is safe, because a replayed row is no longer parked
   and no longer matches (`iam.proto:694-698`).
6. Bulk replay reuses the Cedar action `ReplayOutboxDeadLetter`. There is **no** separate bulk
   action (`application/dead_letters.rs:175`). The wire names `ListOutboxDeadLetters`,
   `ReplayOutboxDeadLetter` and `DiscardOutboxDeadLetter` all exist in the Rust catalog
   (`rs/crates/libs/paigasus-iam-core/src/authz/action.rs:151-153`).
7. A row whose `parked_at` is unset can never satisfy `parked_from` or `parked_to`, because
   Postgres never evaluates a NULL comparison as true. Such a row is invisible to `ListDeadLetters`
   whenever either bound is set, and stays reachable through an unfiltered list (`iam.proto`, the
   comment block above `DeadLetterEntry`).
8. An absent timestamp means unfiltered. A present but unrepresentable one is `INVALID_ARGUMENT`
   and is never silently unfiltered (`iam.proto:662-665`, design D10 of the parent work).
9. `toActionResult` discards the value: `result.ok ? { ok: true } : { ok: false, error }`
   (`ts/packages/paigasus-console-core/src/form.ts:39`). `ActionResult` carries no payload.
10. `bulkReplayDeadLetters` is present on `IamClients['outbox']` today and is called nowhere in
    `ts/apps/iam-console`.
11. `tests/e2e/support/world.ts` registers `outbox.listDeadLetters`, `outbox.replayDeadLetter` and
    `outbox.discardDeadLetter` only. The fake answers every unscripted method with `Unimplemented`,
    so a bulk call fails until a handler exists.
12. `setHandlers` **replaces** the whole handler map; it does not merge.

---

## 4. The parked-time filter

### 4.1 The parser

`ts/apps/iam-console/lib/paging.ts` gets one new exported function, next to `parseEventType`:

```ts
export type ParsedParkedBound = { ok: true; value: string } | { ok: false; error: PaigasusError };

export function parseParkedBound(raw: string | readonly string[] | undefined, field: ParkedBoundField): ParsedParkedBound;
```

`ParkedBoundField` is `'parkedFrom' | 'parkedTo'`. The field only selects the error sentence.

Behaviour:

- It reads the first value when `raw` is an array, using the existing private `first()`.
- It trims the value. An absent or empty value returns `{ ok: true, value: '' }`, which means "no
  filter", exactly as `parseEventType` returns `''`.
- It refuses a value longer than `MAX_PARKED_BOUND_LENGTH` (40 characters). The longest legal
  value the pattern accepts, `2026-09-19T00:00:00.123456789+02:00`, is 35 characters, so the bound
  refuses nothing legal. It stops an attacker-controlled query string from reaching the regular
  expression at length.
- It accepts the value only when **both** hold:
  1. the value matches `/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{1,9})?(Z|[+-]\d{2}:\d{2})$/`;
  2. `Number.isFinite(Date.parse(value))`.
- Any other value returns `{ ok: false, error: parkedBoundInvalid(field) }`, an `invalid-input`
  `PaigasusError` built like the existing `cursorTooLong()` and `eventTypeTooLong()`: no correlation
  id, because it never reached IAM.

The second test is not redundant. The regular expression accepts `2026-02-30T00:00:00Z`, and
`Date.parse` refuses it. The regular expression is what refuses a value with no zone, which
`Date.parse` would silently read as local time.

**The parser never truncates and never guesses a zone.** That follows `parseCursor`'s philosophy: a
value the console will not forward is a reported error, not a silent change.

Error sentences:

- `parkedFrom`: `The "parked from" filter is not a valid time. Use an ISO instant with a zone, for example 2026-09-19T00:00:00Z.`
- `parkedTo`: the same with `"parked to"`.

### 4.2 The page

`page.tsx` parses both bounds beside the two existing parsers:

```ts
const eventType = parseEventType(query.eventType);
const parkedFrom = parseParkedBound(query.parkedFrom, 'parkedFrom');
const parkedTo = parseParkedBound(query.parkedTo, 'parkedTo');
const cursor = parseCursor(query.cursor);
```

The frame key becomes:

```ts
const frameKey = `${ok(eventType)}|${ok(parkedFrom)}|${ok(parkedTo)}|${ok(cursor)}`;
```

where a refused parser contributes `''`, as today. A change of any filter or of the cursor therefore
starts a new, empty frame, and no result from the previous scope remains.

All four parsers run **before** `iamClients()` is built, so a refused query never becomes an IAM
call. The refusal order is `eventType`, `parkedFrom`, `parkedTo`, `cursor`, and the first refusal
renders `PageError`. The degraded branch still comes first and still wins whatever the query says.

The page does **not** compare the two bounds. An inverted window (`from` after `to`) is a legal
request that matches nothing, and IAM owns that judgement. The console adds no rule IAM does not
have.

### 4.3 The filter form

`filterForm` takes the three filter values and renders three text inputs plus the existing submit
button. It stays a `method="get"` form with **no `action` attribute**, so it submits to the current
URL and stays inside the zone. It needs no JavaScript.

Each parked input carries `placeholder="2026-09-19T00:00:00Z"`, `autoComplete="off"` and
`maxLength={MAX_PARKED_BOUND_LENGTH}`.

A refused bound renders `PageError`, so the form is not on screen to show a per-field error. That
matches how a refused cursor behaves today.

### 4.4 The loader

`loadDeadLettersPage` takes `parkedFrom` and `parkedTo` as strings. For each one it sends:

- `undefined` when the string is `''`, so the field is absent and the filter is off;
- `timestampFromDate(new Date(value))` from `@bufbuild/protobuf/wkt` otherwise.

The page already refused every value that `new Date` cannot parse, so the conversion cannot throw.

### 4.5 Paging and the empty-value rule

Both links pass all three filters to `listHref`, which already omits an empty value:

```ts
listHref(PATH, { eventType, parkedFrom, parkedTo })                 // First page
listHref(PATH, { eventType, parkedFrom, parkedTo, cursor: next })   // Next
```

An unfiltered first page therefore still has no query string. This is AC 3.

### 4.6 The NULL `parked_at` caveat

Fact 7 has an operator-visible consequence: with either bound set, a row that is not parked is
invisible. The table caption gains one sentence so that an operator does not read a filtered empty
list as an empty queue:

> Newest events first. With a parked-time filter set, an event with no parked time is not listed.

---

## 5. Carrying the count back

`ActionResult` has no payload (fact 9), so `replayed` cannot travel through it. The bulk action gets
its own state type, declared in `commands.ts`:

```ts
export type BulkReplayResult = { readonly ok: true; readonly replayed: number } | { readonly ok: false; readonly error: PaigasusError };
export type BulkReplayState = BulkReplayResult | null;
export type BulkReplayAction = (previous: BulkReplayState, form: FormData) => Promise<BulkReplayState>;
```

`{ ok: true, replayed }` is structurally assignable to `ActionResult`, so **nothing in
`paigasus-console-core` changes**. Widening the shared `ActionResult` was rejected: it is used by the
gateway console too, and this is the only caller that needs a payload.

The command converts IAM's `bigint` to a `number` with `Number(response.replayed)`. The value is at
most 10000 (fact 4), so the conversion is exact and no bigint crosses the Server Action boundary.

---

## 6. The bulk-replay form

### 6.1 Where the code lives

`dead-letters-frame.tsx` is 186 lines and already holds the frame and the row controls. The bulk
form goes in a new file rather than growing it further:

- **`dead-letters-frame.tsx`** changes:
  - `DeadLetterControl` becomes `'replay' | 'discard' | 'bulk-replay'`.
  - `DeadLetterActions` gains `bulkReplay: BulkReplayAction`.
  - `FrameResult`'s `answer` case splits, because the two answers name different things:
    ```ts
    | { kind: 'answer'; control: 'replay' | 'discard'; id: string; state: ActionResult }
    | { kind: 'bulk-answer'; state: BulkReplayResult }
    ```
  - `Runner.run` widens so that it does not read `id` for a bulk submission. The generation counter,
    the `busy` lock and the rejected-promise path are unchanged and cover the bulk form too.
  - `useRunner` becomes exported, so the new file can consume the context.
- **`bulk-replay-form.tsx`** (new, `'use client'`) holds `BulkReplayForm` and the exported pure
  function `bulkReplayConfirmation`.

The frame stays the only result region, and no control holds its own result. That is the rule SMA-629
§ 6.4 set, and the issue restates it: the result goes to the frame, never to a row control.

### 6.2 Where the form renders

The page renders `BulkReplayForm` **only when the list read succeeded** — with the table and with the
empty list. It does not render in the degraded view and not beside a `SectionError`.

The reason is D1: the table is the evidence for the scope. When the list read failed, the scope is
unverified, and a destructive control with no evidence behind it is exactly what D1 removes.

`mayI('ReplayOutboxDeadLetter', ROOT_PRN)` false also removes it (§ 8).

### 6.3 The shape

```
─ Bulk replay ───────────────────────────────
Scope: event type iam.tenant.created,
       parked 2026-09-19T00:00:00Z → (no end)
Max rows [      ]   (Bulk replay…)
```

- The scope is **read-only text**, built by a pure exported function `bulkReplayScopeText`. The same
  three values go out as `<input type="hidden">` fields, so no JavaScript is needed to submit them.
- `max_rows` is `<input type="number" min={1} max={MAX_BULK_REPLAY_ROWS} step={1} required>`. The
  HTML attributes are a convenience only. § 6.5 is the guard.
- The submit button is disabled while `runner.busy` is true, as every row control is.

### 6.4 The confirmation

The two-step pattern of `ArchiveButton` (`app/_components/lifecycle-button.tsx:42-80`), which
`DeadLetterRowControls` already follows: a first button, then the confirmation text with a
"Confirm bulk replay" submit button and a "Cancel" button that clears the step.

`bulkReplayConfirmation({ eventType, parkedFrom, parkedTo, maxRows })` is exported and unit tested,
as `discardConfirmation` is. It names four things:

1. **The budget, exactly.** "Replay up to 500 parked events." The number is the operator's own,
   because § 6.5 refused anything IAM would clamp. This is AC 2.
2. **The scope in words.** "Scope: event type `iam.tenant.created`, parked from 2026-09-19T00:00:00Z."
   An absent bound is named as absent. An empty scope reads "The scope is EVERY parked event: no
   event type and no parked-time window." (D5.)
3. **That the scope reaches past the page.** "The scope can hold events this page does not show. A
   page holds 50 events, and the budget can be larger."
4. **The two risks.** "A replay can publish an event a second time when the broker deduplication
   window has passed. A cancelled or timed-out request can leave an unknown number of events already
   replayed; running it again is safe, because a replayed event is no longer parked."

Point 4 is a change from SMA-629, which rejected a deduplication warning on single replay on grounds
of scope (§ 12, a QUESTION row, recorded as a risk in its § 9). It is included here because the
blast radius is the stated reason this confirmation exists. Sven approved it on 2026-09-21.

### 6.5 Validation

`commands.ts` gains:

```ts
export const bulkReplayForm = z.object({
  eventType: z.string().trim().max(MAX_EVENT_TYPE_LENGTH),
  parkedFrom: parkedBoundField,
  parkedTo: parkedBoundField,
  maxRows: z.coerce.number().int().min(1).max(MAX_BULK_REPLAY_ROWS),
});
```

`parkedBoundField` is a zod schema that accepts `''` or a value the § 4.1 regular expression and
`Date.parse` both accept. It is built from the same exported regular expression, so the GET filter
and the POST form cannot drift apart.

- An **empty or absent** `max_rows` coerces to `0` and fails `min(1)`. The action answers
  `invalidFormInput()` and never builds a request. That is **AC 1**.
- A value **above 10000** fails `max()` and answers the same way.
- `invalidFormInput()`'s sentence is generic ("Fill in every field of the form."), so it cannot name
  the range. `BulkReplayForm` therefore renders the range as **static helper text** under the input,
  always, not only after a refusal: "Enter a whole number from 1 to 10000." Nothing conditional
  depends on the error, so the operator reads the bound before they type.

`MAX_BULK_REPLAY_ROWS = 10_000` lives in `lib/form.ts`, beside the console's other input bounds,
with a comment citing `dead_letter.rs:77`. It is not a paging bound, so it does not belong in
`lib/paging.ts`. § 7.2 pins it.

### 6.6 The command and the action

```ts
export async function bulkReplayDeadLetters(
  deps: { outbox: Pick<IamClients['outbox'], 'bulkReplayDeadLetters'> },
  input: BulkReplayInput,
): Promise<BulkReplayResult>;
```

It builds the request exactly as § 4.4 builds the list request: an absent timestamp for an empty
bound, `timestampFromDate` otherwise, and `BigInt(input.maxRows)` for `max_rows`. It calls
`callIam`, and on success returns `{ ok: true, replayed: Number(value.replayed) }`. On failure it
returns `{ ok: false, error }`, the same shape `toActionResult` produces.

`actions.ts` gains `bulkReplayDeadLettersAction`, following the existing two exactly:
`iamClientsForAction()` first, then a zod parse through
`formFields(form, ['eventType', 'parkedFrom', 'parkedTo', 'maxRows'])`, then `invalidFormInput()` on
a parse failure, then the command.

**Revalidation.** `refreshesAfterDeadLetterAction(result)` already returns `true` for `ok` and for a
`not-found` failure, and `false` otherwise. Bulk replay never answers `not-found`, so the helper is
reused unchanged: a successful bulk replay revalidates, and every failure does not. A successful
bulk replay with `replayed: 0` still revalidates; the cost is one list read and the benefit is that
the operator sees the current queue.

### 6.7 The result text

The frame's `ResultMessage` gains the `bulk-answer` case:

- `replayed > 0`: "Replayed 8 events."
- `replayed === 0`: "Replayed 0 events. No parked event matched the scope."

Zero is a real and useful answer, not a failure. A failure renders `FormError`. The `DEAD_LETTER_GONE`
sentence stays on the two single-row actions only, because bulk replay cannot answer `not-found`.

The rejected-promise path is the frame's existing `unreached` case, whose text already says that the
result is unknown and that the operator must reload. That text is correct for bulk replay too, and
fact 5 makes it more important: a rejected bulk call can have replayed an unknown number of events.

---

## 7. Tests

### 7.1 Unit

| File | What |
|---|---|
| `tests/unit/paging.test.ts` | `parseParkedBound`: `undefined`, `''`, `'   '`, a valid `Z` instant, a valid `+02:00` instant, a fractional-second instant, an array (first value wins), a value with **no zone**, `2026-02-30T00:00:00Z` (regular expression passes, `Date.parse` refuses), `'not a time'`, and a value over `MAX_PARKED_BOUND_LENGTH`. Both `parkedFrom` and `parkedTo` error sentences. |
| `tests/unit/paging.test.ts` | `listHref` with the two new keys, proving an empty bound is omitted. |
| `tests/unit/bulk-replay-form.test.tsx` (new) | `bulkReplayConfirmation`: the full scope, one bound only, the empty scope (D5), the exact budget number. `bulkReplayScopeText`. The two-step confirm and Cancel. The submit button disabled while `runner.busy`. |
| `tests/unit/dead-letters-frame.test.tsx` | The `bulk-answer` result case, the zero-row sentence, and that a bulk submission does not read `id`. |
| `tests/unit/dead-letters-page.test.ts` | Four new branches: `mayI` false hides the bulk form and row Replay and keeps Discard; a refused `parkedFrom` renders `PageError`; the parked bounds reach the loader; the paging links carry the bounds. |

### 7.2 The drift test (new file)

`ts/apps/iam-console/tests/unit/bulk-replay-ceiling.test.ts` reads
`rs/crates/libs/paigasus-iam-core/src/dead_letter.rs` as text and asserts that
`MAX_BULK_REPLAY_ROWS` equals the Rust constant. It follows
`ts/packages/paigasus-console-core/tests/unit/action-names.test.ts`, which already reads Rust source
as text.

It carries the same **floor assertion** that test carries: a changed source format must fail the
test loudly, not match nothing and pass vacuously. Concretely, the regular expression match is
asserted to exist before its value is compared, and the parsed value is asserted to be a finite
number greater than zero.

### 7.3 `action-names.test.ts`

`tests/unit/action-names.test.ts` iterates `IAM_ACTIONS` with `it.each`, so adding
`ReplayOutboxDeadLetter` to that array pins the new name with no edit to the test.

### 7.4 Integration

`tests/integration/dead-letter-commands.test.ts` gains cases for `bulkReplayDeadLetters` against the
fake IAM:

- a success, asserting the `replayed` count reaches the caller as a `number`;
- the request IAM received, asserting `eventType`, `parkedFrom`, `parkedTo` and `maxRows` — the
  timestamps as absent for an empty bound and as the right instant otherwise;
- a denial, asserting the `forbidden` presentation reaches the caller.

`tests/integration/dead-letters-page.test.ts` gains one case proving the parked bounds reach IAM on
the list call.

### 7.5 e2e

`tests/e2e/support/world.ts` gains an `outbox.bulkReplayDeadLetters` handler. It filters the seeded
map by `eventType` and by the parked window, takes at most `maxRows` rows, removes them, and answers
`{ replayed: BigInt(n) }`. It follows the filtering style of the existing `outbox.listDeadLetters`
handler. Without it the call answers `Unimplemented` (fact 11).

The seeded set needs a third entry so that a bulk replay can act on more than one row and leave one
behind. The new entry gets a distinct `eventType` and a `parkedAt` outside the window the test uses.

`tests/e2e/dead-letters.spec.ts` gains **R20**: set a parked-time filter, assert the request the fake
received, follow a paging link and assert the filter survived it (AC 3), then open the bulk form,
confirm, and assert both the result text and the request the fake recorded (AC 4).

The test waits through `tests/e2e/support/hydration.ts`'s `waitForHydration` before the first click,
as `dead-letters.spec.ts:17` does. The bulk confirm step needs JavaScript, exactly as discard does.

---

## 8. Authorization

`ts/packages/paigasus-console-core/src/authorize.ts`: `IAM_ACTIONS` gains
`'ReplayOutboxDeadLetter'`. The comment that says the console asks no question about replay or
discard is rewritten: it now asks about replay, and still asks nothing about discard.

`app/(console)/dead-letters/page.tsx` calls `mayI('ReplayOutboxDeadLetter', ROOT_PRN)` itself. The
layout cannot pass the answer down, because a layout's `children` are opaque to it.

This reverses the page's header comment (`page.tsx:3-5`), which states that the page asks discovery
and not `mayI`. **That comment and its reasoning are rewritten, not only the code.** The new comment
states: the page asks discovery for the gate and `mayI` for the replay affordance; `mayI` is
cosmetic and fails open; IAM decides every action.

The answer is fetched in the same `Promise.all` as the discovery call, so it adds no serial
round trip. It is fetched **after** the `not-found` gate, so a hidden screen asks no authorization
question.

The commands and actions take no `mayI`. That rule is unchanged: IAM decides.

---

## 9. Risks

| # | Risk | Position |
|---|---|---|
| R1 | A bulk replay can publish duplicate events past the broker deduplication window, multiplied by the row budget. | Accepted, and named in the confirmation (§ 6.4 point 4). The console cannot detect it. |
| R2 | A cancelled or timed-out bulk RPC leaves an unknown number replayed. | Accepted, and named in the confirmation. Re-running is safe (fact 5). The frame's `unreached` text already tells the operator to reload. |
| R3 | `MAX_BULK_REPLAY_ROWS` duplicates a Rust constant and can drift. | The drift test in § 7.2 fails when it drifts. |
| R4 | `mayI` fails open, so D4's affordance is not a guard. | Recorded in § 2.4. IAM is the guard. |
| R5 | The bulk form acts on rows the operator never saw, because the budget can exceed the page size. | Named in the confirmation (§ 6.4 point 3). D1 keeps the scope equal to the filter, which is the strongest link the design can make between the table and the button. |
| R6 | A row with no `parked_at` disappears from a filtered list. | Named in the table caption (§ 4.6). It is a property of IAM's query, not of the console. |

---

## 10. Out of scope

- Any proto, Rust or HTTP change. Everything this design needs exists (§ 1).
- A progress indicator or a partial-count report for a long bulk replay. The RPC answers once.
- A separate Cedar action for bulk replay. IAM has none (fact 6).
- `DiscardOutboxDeadLetter` in `IAM_ACTIONS` (D4).
- A bulk discard. The issue does not ask for one, and IAM has no such RPC.

---

## 11. Files

| File | Change |
|---|---|
| `ts/apps/iam-console/lib/paging.ts` | `parseParkedBound`, `PARKED_BOUND_PATTERN`, `MAX_PARKED_BOUND_LENGTH`, `parkedBoundInvalid()` |
| `ts/apps/iam-console/lib/form.ts` | `MAX_BULK_REPLAY_ROWS` |
| `ts/apps/iam-console/app/(console)/dead-letters/page.tsx` | two parsers, the frame key, the filter form, the loader call, the paging links, `mayI`, the header comment |
| `ts/apps/iam-console/app/(console)/dead-letters/load.ts` | `parkedFrom` and `parkedTo` on the list request |
| `ts/apps/iam-console/app/(console)/dead-letters/commands.ts` | `bulkReplayForm`, `parkedBoundField`, `BulkReplayResult`, `bulkReplayDeadLetters` |
| `ts/apps/iam-console/app/(console)/dead-letters/actions.ts` | `bulkReplayDeadLettersAction` |
| `ts/apps/iam-console/app/(console)/dead-letters/dead-letters-frame.tsx` | the widened control, result union and runner; exported `useRunner`; the bulk result text |
| `ts/apps/iam-console/app/(console)/dead-letters/bulk-replay-form.tsx` | **new** |
| `ts/apps/iam-console/app/(console)/dead-letters/dead-letter-table.tsx` | the caption sentence (§ 4.6) |
| `ts/packages/paigasus-console-core/src/authorize.ts` | `ReplayOutboxDeadLetter` in `IAM_ACTIONS`, and the comment |
| `ts/apps/iam-console/tests/...` | § 7.1, § 7.2, § 7.4, § 7.5 |
| `ts/apps/iam-console/tests/e2e/support/world.ts` | the bulk handler and a third seeded entry |
