# SMA-661 — bulk replay and the parked-time filter on `/iam/dead-letters`

- **Issue:** SMA-661, a follow-up from SMA-629 (spec § 11, decision D1).
- **Status:** revision 2. Sven approved the design in chat on 2026-09-21. Revision 2 folds in the
  Stage 2 challenge (§ 12) and adds decision D6.
- **Parent spec:** `docs/superpowers/specs/2026-09-19-sma-629-dead-letters-capability-design.md`
  (§ 5.3, § 6.3, § 6.4, § 6.5, § 7, § 9, § 11).

---

## 1. Problem

SMA-629 delivered the `/iam/dead-letters` screen: a list, a single replay and a single discard.
Decision D1 of that spec put two things out of scope, and this issue delivers them:

1. **A parked-time filter on the list.** `ListDeadLettersRequest` carries `parked_from` and
   `parked_to` (`contracts/proto/paigasus/iam/v1/iam.proto:666-672`). The console sends neither.
   An operator cannot narrow the list to the incident window.
2. **Bulk replay.** `BulkReplayDeadLetters` exists on `OutboxService` (`iam.proto:720`). No console
   code calls it. An operator must replay a large backlog one row at a time.

Nothing in the backend changes. The proto, the Rust service and the generated TypeScript client all
carry both features already. The work is in `ts/apps/iam-console`, in
`ts/packages/paigasus-console-core` (one action name, one constant, one drift test), and in the e2e
world.

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
| D2 | How a parked bound is written | A **text input** that needs an ISO 8601 instant **with a zone**. A value with no zone is refused. |
| D3 | The 10000 ceiling | The console **refuses** `max_rows` outside `1..10000`. A unit test pins the bound to IAM's Rust constant. |
| D4 | Authorization | The page asks about `ReplayOutboxDeadLetter` at Root. A `false` answer hides the **bulk form and the row Replay buttons**. Discard is unchanged. |
| D5 | An empty scope | **Allowed.** The row budget is the only guard, as the proto states. The confirmation names the empty scope in words. |
| D6 | The cost of a typo | The parser accepts four spellings and normalises them. A refused bound **re-renders the filter form** with the typed value and a per-field error. It does not replace the page. |

### 2.1 Why D1

The screen's value is that the table is the evidence for what the button does. A bulk form with its
own inputs can name a scope the table never showed, on an action that re-injects up to 10000 events.
Binding the form to the filter stops the operator from typing a second, unseen scope.

**What D1 is not.** The binding is a user-interface convention, not a server-enforced property. The
scope travels as hidden fields, and nothing on the server compares it with the list the operator
read. A Root caller can always post any scope. See R7.

### 2.2 Why D2 and D6

A `datetime-local` input carries no zone. The browser shows the operator their local time, and the
server reads the same digits as UTC. The window the confirmation named would then not be the window
the operator saw. A hidden offset field removes the ambiguity but needs JavaScript, and the filter
form is deliberately a plain GET form that works without it (SMA-629 § 6.3).

A text instant also matches the vocabulary already on screen: `load.ts` maps `parked_at` through
`timestampIso` (`lib/time.ts`), so the table prints ISO instants. The operator types what they read.

D6 is the correction the Stage 2 challenge forced. Requiring the one canonical spelling, and
answering a typo with a whole-page `PageError` that loses the other two filters, makes an
incident-window filter hostile on a screen an operator reaches under pressure. The zone requirement
is the only part of D2 that protects anything, and D6 keeps it whole.

### 2.3 Why D3

`max_rows` above 10000 is silently clamped by IAM, on both transports, with no signal to the caller
(`iam.proto:704-710`). A confirmation that named a larger number would be false, and AC 2 requires
it to be true. Refusing the value keeps the sentence exact.

The cost is a constant duplicated across languages. § 7.2 pins it with a drift test.

### 2.4 Why D4, and what it does not buy

Sven chose to add the authorization question. The recorded counter-argument, which the spec keeps so
that a later reader sees it:

- Every `OutboxService` RPC is Root-only inside IAM. A caller who cannot list gets `forbidden` and
  the page renders `PageError`, so such a caller never sees the frame at all.
- `mayI` is cosmetic and **fails open**: on a failed authorization query, or an unnamed principal,
  it answers `true` and the control shows (`paigasus-console-core/src/authorize.ts:80-88`).

So the question hides a control from nobody who could otherwise have used it, and it cannot be
relied on as a guard. IAM stays the only real decision. The value it does add is an affordance that
matches the caller's permissions when the query succeeds.

D4 reaches the row Replay buttons as well, because they invoke the same Cedar action. Hiding the
bulk form alone would put two contradictory rules for one permission on one page.

`DiscardOutboxDeadLetter` stays out of `IAM_ACTIONS`, so a row can show Discard without Replay. That
is truthful: they are separate permissions.

### 2.5 Why D5

The proto states the rule directly: "the explicit row budget is the guard on blast radius"
(`iam.proto:705-707`). IAM's own type documentation goes further and records that it **considered and
rejected** an "at least one filter field must be present" rule, because
`parked_from = 1970-01-01T00:00:00Z` satisfies such a rule while matching every row, and that is the
most natural way an operator writes "replay everything"
(`rs/crates/libs/paigasus-iam-core/src/dead_letter.rs:60-67`). The console adds no rule IAM examined
and dropped.

---

## 3. Facts this design rests on

Each is measured in the repository. Line numbers were re-derived on 2026-09-21.

1. `BulkReplayDeadLettersRequest` (`iam.proto:700`) holds `event_type`, `parked_from`, `parked_to`,
   `max_rows`. `BulkReplayDeadLettersResponse` holds `replayed` only. The RPC is `iam.proto:720`.
2. `max_rows` is `uint64` and `replayed` is `uint64`. protobuf-es maps both to **`bigint`**
   (`ts/packages/paigasus-proto/src/generated/paigasus/iam/v1/iam_pb.ts`).
3. `max_rows` 0 is rejected before any store access. An absent field collapses to 0, so "did not
   say" and "said zero" are refused identically. This is deliberate (`iam.proto:703-707`).
4. The clamp is `BulkReplayRequest::MAX_BULK_REPLAY = 10_000`
   (`rs/crates/libs/paigasus-iam-core/src/dead_letter.rs:77`). `capped_max_rows()` applies it
   (`:82-84`), and the store binds it (`pg_dead_letters.rs:194`), so `replayed` really is at most
   10000. The audit record keeps `max_rows` and `capped_max_rows` as separate fields
   (`rs/crates/services/paigasus-iam/src/application/dead_letters.rs:183-193`).
5. Bulk replay is **not atomic** (`iam.proto:696-699`). A `DEADLINE_EXCEEDED` or a cancelled RPC can
   leave an unknown number of rows already replayed. Re-issuing is safe, because a replayed row is
   no longer parked and no longer matches.
6. Bulk replay reuses the Cedar action `ReplayOutboxDeadLetter`. There is **no** separate bulk
   action (`application/dead_letters.rs:175`). The wire names `ListOutboxDeadLetters`,
   `ReplayOutboxDeadLetter` and `DiscardOutboxDeadLetter` all exist in the Rust catalog
   (`rs/crates/libs/paigasus-iam-core/src/authz/action.rs:151-153`).
7. A row whose `parked_at` is unset can never satisfy `parked_from` or `parked_to`, because
   Postgres never evaluates a NULL comparison as true. The blind spot covers **`list` and
   `replay_matching_in` alike** (`pg_dead_letters.rs:21-30`;
   `docs/ops/RUNBOOK-observability.md:378-381`).
8. Both bounds are **inclusive**: `parked_at >= $n` and `parked_at <= $n`
   (`pg_dead_letters.rs:100,104`).
9. An absent timestamp means unfiltered. A present but unrepresentable one is `INVALID_ARGUMENT`
   and is never silently unfiltered (`iam.proto:662-665`).
10. `toActionResult` discards the value: `result.ok ? { ok: true } : { ok: false, error }`
    (`ts/packages/paigasus-console-core/src/form.ts:40`). `ActionResult` carries no payload.
    `ActionState` is `{ ok: true } | { ok: false; error: PaigasusError } | null`
    (`paigasus-console-core/src/errors.ts:38`).
11. `mayI` is an **accessor**, not the question. The shape is
    `const may = await mayI(); may('Action', ROOT_PRN)` (`app/(console)/layout.tsx:33-34`). The memo
    inside `createMayI` keys on `action\0resourcePrn` (`authorize.ts:94`), so the layout's
    `ListOutboxDeadLetters` question and a `ReplayOutboxDeadLetter` question do not share a result.
12. `bulkReplayDeadLetters` is present on `IamClients['outbox']` today and is called nowhere in
    `ts/apps/iam-console`.
13. `tests/e2e/support/world.ts` registers `outbox.listDeadLetters`, `outbox.replayDeadLetter` and
    `outbox.discardDeadLetter` only. The list handler filters by `eventType` alone and answers
    `nextCursor: ''` unconditionally. The fake answers every unscripted method with `Unimplemented`.
14. `world.ts`'s `authz.isAuthorized` answers `allowed: allow.has(req.action)`, and `allow` defaults
    to `ALL_ACTIONS` declared in the same file. An action absent from `ALL_ACTIONS` is **denied** in
    every e2e test.
15. `setHandlers` **replaces** the whole handler map; it does not merge. `startFakeIam`'s
    `options.overrides` merges into the world's base map at construction.
16. `ArchiveButton` (`app/_components/lifecycle-button.tsx:48-89`) renders **no form in step 1**.
    The `<form>` exists only in step 2.
17. `paigasus-console-core`'s `test` task already declares Rust source as an input
    (`ts/packages/paigasus-console-core/moon.yml:58,64`), with a comment stating that without it a
    changed Rust action name selects no task and Moon serves a cached pass. `iam-console`'s `test`
    task declares no `/rs/**` input and carries `options.merge: replace`.

---

## 4. The parked-time filter

### 4.1 The parser

`ts/apps/iam-console/lib/paging.ts` gains one exported function, next to `parseEventType`:

```ts
export type ParsedParkedBound =
  | { ok: true; raw: string; iso: string }
  | { ok: false; raw: string; error: PaigasusError };

export function parseParkedBound(raw: string | readonly string[] | undefined, field: ParkedBoundField): ParsedParkedBound;
```

`ParkedBoundField` is `'parkedFrom' | 'parkedTo'`, and it only selects the error sentence. The
result carries **two** strings on purpose:

- `raw` — what the operator typed. The filter input echoes it, so a refusal does not erase it (D6).
- `iso` — the canonical instant, `new Date(raw).toISOString()`. The paging links, the bulk form's
  hidden fields and the confirmation text all use `iso`, so a link is stable and a confirmation
  names one unambiguous spelling. An empty filter has `iso === ''`.

Behaviour:

- It reads the first value when `raw` is an array, using the existing private `first()`.
- It trims the value. An absent or empty value returns `{ ok: true, raw: '', iso: '' }`, which means
  "no filter", exactly as `parseEventType` returns `''`.
- It refuses a value longer than `MAX_PARKED_BOUND_LENGTH` (40 characters). The longest value the
  pattern accepts, `2026-09-19T00:00:00.123456789+02:00`, is 35 characters, so the bound refuses
  nothing legal. It stops an attacker-controlled query string from reaching the pattern at length.
- It accepts the value only when **both** hold:
  1. the value matches `PARKED_BOUND_PATTERN`;
  2. `Number.isFinite(Date.parse(value))`.
- Any other value returns `{ ok: false, raw, error: parkedBoundInvalid(field) }`, an `invalid-input`
  `PaigasusError` built like the existing `cursorTooLong()` and `eventTypeTooLong()`: no correlation
  id, because it never reached IAM.

```ts
export const PARKED_BOUND_PATTERN = /^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}(:\d{2}(\.\d{1,9})?)?(Z|z|[+-]\d{2}:\d{2})$/;
```

So the four spellings of D6 are accepted and normalised: a space instead of `T`, omitted seconds, a
lower-case `z`, and an extended `±HH:MM` offset.

**The basic-format offset `+0200` stays refused**, deliberately. ECMAScript's Date Time String Format
does not include it, so `Date.parse` accepts it only through implementation-specific behaviour. A
parser must not rest on that.

**A zone stays required.** `2026-09-19T00:00:00` and `2026-09-19` are refused. That is the whole of
what D2 protects: without a zone the console would have to guess, and the guess would differ from
what the operator saw.

The two checks are both needed and neither is redundant. The pattern accepts
`2026-02-30T00:00:00Z`, and `Date.parse` refuses it. `Date.parse` accepts a zone-less value and
reads it as local time, and the pattern refuses it.

`MAX_PARKED_BOUND_LENGTH` also goes on the input as `maxLength`. That is a **typing bound only**; it
is not the validation. The parser itself never truncates, in line with `parseCursor`'s rule that a
value the console will not forward is a reported error and not a silent change.

Error sentences, one per field:

- `The "parked from" filter is not a valid time. Use an ISO instant with a zone, for example 2026-09-19T00:00:00Z.`
- the same with `"parked to"`.

**Both bounds are inclusive** (fact 8). § 4.3 and § 6.4 say so in words, because an operator cannot
read it off the input.

### 4.2 The page

`page.tsx` parses both bounds beside the two existing parsers:

```ts
const eventType = parseEventType(query.eventType);
const parkedFrom = parseParkedBound(query.parkedFrom, 'parkedFrom');
const parkedTo = parseParkedBound(query.parkedTo, 'parkedTo');
const cursor = parseCursor(query.cursor);
```

The frame key uses the canonical values, so two spellings of one window share a frame:

```ts
const frameKey = `${eventTypeOr('')}|${parkedFrom.ok ? parkedFrom.iso : ''}|${parkedTo.ok ? parkedTo.iso : ''}|${cursorOr('')}`;
```

All four parsers run **before** `iamClients()` is built, so a refused query never becomes an IAM
call. The degraded branch still comes first and still wins whatever the query says.

**Refusal handling differs by parser, and this is the D6 change.**

| Parser | On refusal |
|---|---|
| `parseEventType`, `parseCursor` | `PageError`, unchanged from SMA-629. |
| `parseParkedBound` (either bound) | The page renders the **normal shell and filter form** with the typed value and a per-field error, and renders **no table and no bulk form**. It makes no IAM call. |

A refused bound is an operator typing mistake, and the other two filter values survive it. A refused
cursor is not hand-typed, so it keeps its whole-page error.

The page does **not** compare the two bounds. An inverted window (`from` after `to`) is a legal
request that matches nothing, and IAM owns that judgement. The console adds no rule IAM does not
have.

### 4.3 The filter form

`filterForm` takes the three filter values and their two possible errors. It renders three text
inputs plus the existing submit button. It stays a `method="get"` form with **no `action`
attribute**, so it submits to the current URL and stays inside the zone. It needs no JavaScript.

Each parked input carries `placeholder="2026-09-19T00:00:00Z"`, `autoComplete="off"` and
`maxLength={MAX_PARKED_BOUND_LENGTH}`, and sits in a `Field` whose `description` reads
`Both bounds are included. Use a zone, for example 2026-09-19T00:00:00Z or +02:00.`

A refused bound passes its sentence to that `Field`'s `error` prop, so `aria-describedby` and
`aria-invalid` are wired by the component (`ts/packages/paigasus-ui/src/components/form.tsx:33,56-58`).

### 4.4 The loader

`loadDeadLettersPage` takes `parkedFrom` and `parkedTo` as the **canonical `iso` strings**. For each
one it sends:

- `undefined` when the string is `''`, so the field is absent and the filter is off;
- `timestampFromDate(new Date(value))` from `@bufbuild/protobuf/wkt` otherwise.

The page already refused every value that `new Date` cannot parse, so the conversion cannot throw.

### 4.5 Paging and the empty-value rule

Both links pass all three filters to `listHref`, which already omits an empty value:

```ts
listHref(PATH, { eventType, parkedFrom: parkedFrom.iso, parkedTo: parkedTo.iso })
listHref(PATH, { eventType, parkedFrom: parkedFrom.iso, parkedTo: parkedTo.iso, cursor: next })
```

An unfiltered first page therefore still has no query string. Following a link converges the URL and
the input on the canonical spelling. This is AC 3.

### 4.6 The NULL `parked_at` caveat

Fact 7 has an operator-visible consequence, and it covers bulk replay as well as the list: with
either bound set, a row that is not parked is invisible to **both**. Two places say so:

- the table caption gains: "Newest events first. With a parked-time filter set, an event with no
  parked time is not listed."
- `bulkReplayConfirmation` adds a sentence whenever either bound is set (§ 6.4 point 5).

---

## 5. Carrying the count back

`ActionResult` has no payload (fact 10), so `replayed` cannot travel through it. The bulk action gets
its own state type, declared in `commands.ts`:

```ts
export type BulkReplayResult = { readonly ok: true; readonly replayed: number } | { readonly ok: false; readonly error: PaigasusError };
export type BulkReplayState = BulkReplayResult | null;
export type BulkReplayAction = (previous: BulkReplayState, form: FormData) => Promise<BulkReplayState>;
```

A value of type `BulkReplayResult` is assignable to `ActionResult` — `ActionState`'s success arm is
`{ ok: true }` (fact 10) and a non-fresh value gets no excess-property check — so
`refreshesAfterDeadLetterAction(result)` compiles unchanged.

**Nothing in `paigasus-console-core`'s `ActionResult`, `FormAction` or `toActionResult` changes.**
That package does change in two other ways, in § 6.5 and § 8; the earlier revision's flat claim that
it did not change was wrong.

The command converts IAM's `bigint` to a `number` with `Number(response.replayed)`. The value is at
most 10000 (fact 4), so the conversion is exact and no bigint crosses the Server Action boundary.

---

## 6. The bulk-replay form

### 6.1 The exact frame change

`dead-letters-frame.tsx` is 185 lines and already holds the frame and the row controls. The bulk
form goes in a new file rather than growing it.

**`DeadLetterControl` does not widen.** It stays `'replay' | 'discard'`, so `successText`
(`dead-letters-frame.tsx:42-44`) keeps its meaning and cannot answer "Discarded" for a bulk
submission, and `actions[control]` keeps its index type. The runner gains a **second method**
instead:

```ts
type Runner = {
  readonly busy: boolean;
  run(control: DeadLetterControl, form: FormData): void;
  runBulk(form: FormData): void;
};
```

`runBulk` shares `generationRef` and `startWork` with `run`, so the generation counter, the shared
busy lock and the rejected-promise path all cover the bulk form. It reads no `id`. It calls
`actions.bulkReplay(null, form)` and sets `{ kind: 'bulk-answer', state }`.

`FrameResult` gains one arm and the existing one is narrowed to the two row controls:

```ts
type FrameResult =
  | null
  | { readonly kind: 'answer'; readonly control: DeadLetterControl; readonly id: string; readonly state: ActionResult }
  | { readonly kind: 'bulk-answer'; readonly state: BulkReplayResult }
  | { readonly kind: 'unreached'; readonly error: PaigasusError };
```

`DeadLetterActions` gains `bulkReplay: BulkReplayAction`. The field is **required**, and
`tests/unit/dead-letters-frame.test.tsx:81-87` builds that object by hand, so § 7.1 lists that file.

`useRunner` becomes exported, so `bulk-replay-form.tsx` can consume the context.

### 6.2 Where the form renders

The page renders `BulkReplayForm` when **all** of these hold:

1. the list read succeeded — so not in the degraded view, not beside a `SectionError`, and not on
   the D6 refused-bound view;
2. `may('ReplayOutboxDeadLetter', ROOT_PRN)` answered `true` (§ 8).

Condition 1 follows D1: the table is the evidence for the scope, and an unverified scope has no
evidence.

**The form does render over an empty list.** The empty state means the current page matched nothing,
and the operator may be on a later page or about to widen the filter. The alternative — hiding it —
adds a branch whose only effect is to hide a control that would truthfully answer "Replayed 0
events." § 6.4 point 3 already tells the operator that the scope reaches past the page.

**Position.** Directly under `filterForm` and above the table or empty state, in its own bordered
section with an `<h2>Bulk replay</h2>`. It sits with the filter that defines it, not with the rows.

### 6.3 The shape

```
─ Bulk replay ───────────────────────────────
Scope: event type iam.tenant.created,
       parked 2026-09-19T00:00:00Z → (no end)
Max rows [      ]   (Bulk replay…)
  Enter a whole number from 1 to 10000.
```

- The scope is **read-only text**, built by the pure exported function `bulkReplayScopeText`. The
  same three canonical values also go out as `<input type="hidden">` fields.
- `max_rows` is a **controlled** input: `BulkReplayForm` holds it in `useState`. The confirmation
  text needs the value, so it cannot live only in the DOM.
- The range is `Field`'s `description` prop, rendered **always**, not only after a refusal, so the
  operator reads the bound before they type. `invalidFormInput()`'s sentence is generic ("Fill in
  every field of the form.") and cannot carry it.
- **One `<form>` wraps both steps.** This differs from `ArchiveButton`, which renders no form in
  step 1 (fact 16). Here the visible input and the three hidden fields must be inside the element
  that submits, so a single form spans the first button, the confirmation text and the confirm
  button.
- The form submits through `onSubmit` with `preventDefault`, calling `runner.runBulk(new FormData(event.currentTarget))`,
  exactly as `DeadLetterRowControls` does (`dead-letters-frame.tsx:135-141`). It **never** uses
  `useActionState`: the frame owns the only result region. The hidden fields need no client-side
  computation, but the submission itself needs JavaScript, as discard does.
- Both buttons are disabled while `runner.busy` is true, as every row control is.

### 6.4 The confirmation

Two steps, as `DeadLetterRowControls` does: a "Bulk replay…" button, then the confirmation text with
a "Confirm bulk replay" submit button and a "Cancel" button that clears the step.

**The first button is disabled unless the `max_rows` state matches `/^\d{1,5}$/` with a value from 1
to `MAX_BULK_REPLAY_ROWS`.** Without this the operator could open a confirmation reading "Replay up
to  parked events." and only then meet a generic form error. AC 2 requires the confirmation to name
the number, so the confirmation must not be reachable without one.

`bulkReplayConfirmation({ eventType, parkedFrom, parkedTo, maxRows })` is exported and unit tested,
as `discardConfirmation` is. It names five things:

1. **The budget, exactly.** "Replay up to 500 parked events." The number is the operator's own,
   because § 6.5 refused anything IAM would clamp. Singular: "Replay up to 1 parked event." This is
   AC 2.
2. **The scope in words**, with canonical instants and the inclusive bounds: "Scope: event type
   `iam.tenant.created`, parked from 2026-09-19T00:00:00Z, both bounds included." An absent bound is
   named as absent. An empty scope reads "The scope is EVERY parked event: no event type and no
   parked-time window." (D5.)
3. **That the scope reaches past the page.** "The scope can hold events this page does not show. A
   page holds 50 events, and the budget can be larger."
4. **The two risks.** "A replay can publish an event a second time when the broker deduplication
   window has passed. A cancelled or timed-out request can leave an unknown number of events already
   replayed; running it again is safe, because a replayed event is no longer parked."
5. **The NULL blind spot, only when either bound is set.** "An event with no parked time is not in
   this scope." (Fact 7.)

Point 4 is a change from SMA-629, which rejected a deduplication warning on single replay on grounds
of scope (§ 12 of that spec, a QUESTION row, recorded as a risk in its § 9). It is included here
because the blast radius is the stated reason this confirmation exists. Sven approved it on
2026-09-21.

### 6.5 Validation

`commands.ts` gains:

```ts
export const bulkReplayForm = z.object({
  eventType: z.string().trim().max(MAX_EVENT_TYPE_LENGTH),
  parkedFrom: parkedBoundField,
  parkedTo: parkedBoundField,
  maxRows: z
    .string()
    .trim()
    .regex(/^\d{1,5}$/)
    .transform(Number)
    .pipe(z.number().int().min(1).max(MAX_BULK_REPLAY_ROWS)),
});
```

`z.coerce.number()` was rejected. It accepts `'0x10'` as 16, `'0b1010'` as 10, `'0o17'` as 15, `'+7'`
as 7 and `'7.'` as 7, so the number IAM receives could differ from the string the confirmation
showed. The digits-only regular expression is the same rule § 6.4 gates the confirmation on, so the
two cannot disagree.

`parkedBoundField` accepts `''`, `null` and `undefined` as "no filter" through a `preprocess`, and
otherwise applies `PARKED_BOUND_PATTERN` plus the `Date.parse` check and normalises with
`toISOString()`. It is built from the **same exported pattern** as § 4.1, so the GET parser and the
POST field cannot drift apart. Treating an absent field as "no filter" matters because `formFields`
returns `form.get(name)`, which is `null` for a field a hand-built POST omitted
(`paigasus-console-core/src/form.ts:47-49`); an omitted optional filter must not read as a missing
required field.

- An **empty or absent** `max_rows` fails the regular expression. The action answers
  `invalidFormInput()` and never builds a request. That is **AC 1**.
- A value **above 10000**, or with more than five digits, fails too.

`MAX_BULK_REPLAY_ROWS = 10_000` lives in `ts/packages/paigasus-console-core/src/form.ts`, beside
`NAME_MAX_CODE_POINTS` (`:19`), and is re-exported from the package index. It is **not** in
`iam-console/lib/`, and that placement is load-bearing: see § 7.2.

### 6.6 The command and the action

```ts
export async function bulkReplayDeadLetters(
  deps: { outbox: Pick<IamClients['outbox'], 'bulkReplayDeadLetters'> },
  input: BulkReplayInput,
): Promise<BulkReplayResult>;
```

It builds the request as § 4.4 builds the list request: an absent timestamp for an empty bound,
`timestampFromDate` otherwise, and `BigInt(input.maxRows)` for `max_rows`. It calls `callIam`, and
on success returns `{ ok: true, replayed: Number(value.replayed) }`. On failure it returns
`{ ok: false, error }`, the shape `toActionResult` produces.

`actions.ts` gains `bulkReplayDeadLettersAction`, following the existing two exactly:
`iamClientsForAction()` first, then a zod parse through
`formFields(form, ['eventType', 'parkedFrom', 'parkedTo', 'maxRows'])`, then `invalidFormInput()` on
a parse failure, then the command.

**Revalidation.** `refreshesAfterDeadLetterAction(result)` returns `true` for `ok` and for a
`not-found` failure, and `false` otherwise (`lib/form.ts:85-87`). Bulk replay never answers
`not-found`, so the helper is reused unchanged: success revalidates, every failure does not. A
successful bulk replay with `replayed: 0` still revalidates; the cost is one list read and the
benefit is that the operator sees the current queue. See R8 for what a revalidated `forbidden` list
read costs.

### 6.7 The result text

The frame's `ResultMessage` gains the `bulk-answer` case:

- `replayed > 1`: "Replayed 8 events."
- `replayed === 1`: "Replayed 1 event."
- `replayed === 0`: "Replayed 0 events. No parked event matched the scope."

Zero is a real and useful answer, not a failure. A failure renders `FormError`. The
`DEAD_LETTER_GONE` sentence stays on the two single-row actions only, because bulk replay cannot
answer `not-found`.

The rejected-promise path is the frame's existing `unreached` case, whose text already says that the
result is unknown and that the operator must reload. That text is correct for bulk replay too, and
fact 5 makes it more important: a rejected bulk call can have replayed an unknown number of events.

---

## 7. Tests

### 7.1 Unit

| File | Change |
|---|---|
| `iam-console/tests/unit/paging.test.ts` | `parseParkedBound`: `undefined`, `''`, `'   '`, canonical `Z`, lower-case `z`, a space separator, omitted seconds, fractional seconds, a `+02:00` offset, a basic-format `+0200` (**refused**), an array (first value wins), **no zone** (refused), `2026-02-30T00:00:00Z` (pattern passes, `Date.parse` refuses), `'not a time'`, over `MAX_PARKED_BOUND_LENGTH`. Each accepted case asserts `iso` is the canonical normalisation, and each refused case asserts `raw` is echoed back. Both field sentences. `listHref` with the two new keys, proving an empty bound is omitted. |
| `iam-console/tests/unit/bulk-replay-form.test.tsx` (**new**) | `bulkReplayConfirmation`: full scope, one bound only, the empty scope (D5), the singular budget, the NULL-blind-spot sentence present with a bound and absent without one. `bulkReplayScopeText`. The two-step confirm and Cancel. The first button disabled for `''`, `'0'`, `'10001'`, `'7.5'`, `'0x10'`. Both buttons disabled while `runner.busy`. |
| `iam-console/tests/unit/dead-letters-frame.test.tsx` | The `bulk-answer` arm and the three count sentences. That `runBulk` reads no `id` and still honours the generation counter. **Every existing case needs the new required `bulkReplay` field** in the hand-built `DeadLetterActions` at `:81-87`. |
| `iam-console/tests/unit/dead-letters-page.test.ts` | The `lib/console` mock at `:20-25` gains `mayI`, or the page throws. New branches: `may` false hides the bulk form and row Replay and keeps Discard; a refused `parkedFrom` renders the filter form with the typed value and no table and makes no IAM call; the canonical bounds reach the loader; the paging links carry them. |
| `iam-console/tests/unit/e2e-rows.test.ts` | `length: 19` → `20` at `:15`, and the header comment names this spec. Its second case reds on an unlisted row. |
| `iam-console/tests/unit/actions-structure.test.ts` | `EXPECTED` at `:26` gains `bulkReplayDeadLettersAction`. `:141` asserts strict equality on the exported names. |
| `iam-console/tests/unit/world-actions.test.ts` | `:12` asserts `[...ALL_ACTIONS].sort()` equals `[...IAM_ACTIONS].sort()`, so `world.ts`'s `ALL_ACTIONS` must gain the new name. |
| `console-core/tests/unit/action-names.test.ts` | `:65-69` currently asserts `not.toContain('ReplayOutboxDeadLetter')`. Move that name to the positive arm and keep `DiscardOutboxDeadLetter` negative. Rewrite the comment above it. |

### 7.2 The drift test, and why it lives in `console-core`

`MAX_BULK_REPLAY_ROWS` and its drift test go in `@paigasus/console-core`, **not** in `iam-console`.

Fact 17 is the reason. `iam-console`'s `test` task declares no `/rs/**` input and carries
`options.merge: replace`, so an edit to `dead_letter.rs` selects no `iam-console-ts` task and Moon
serves a cached pass — on exactly the change the test exists to catch. `console-core`'s `test` task
already declares Rust source as an input for the same reason, with a comment saying so.

So: `ts/packages/paigasus-console-core/moon.yml` gains
`- '/rs/crates/libs/paigasus-iam-core/src/dead_letter.rs'` beside the two existing Rust inputs, and
the test is `ts/packages/paigasus-console-core/tests/unit/bulk-replay-ceiling.test.ts`.

The alternative — adding the Rust input to `iam-console-ts:test` — was rejected because that task
depends on `~:build`, so every `dead_letter.rs` edit would rebuild the console.

**The test must not pass vacuously.** The file's own doc comment at
`dead_letter.rs:60-62` contains the prose `MAX_BULK_REPLAY` and `(10_000)`, so a loose regular
expression extracts `10_000` from a comment with the constant **deleted**. The test therefore:

- matches `/^\s*pub const MAX_BULK_REPLAY: u64 = ([0-9_]+);/m`;
- asserts there is **exactly one** match in the file;
- strips `_` before `Number()` — note `Number('10_000')` is `NaN`, so the strip is required and its
  absence fails loudly;
- asserts the parsed value is a finite integer greater than zero, then compares it with
  `MAX_BULK_REPLAY_ROWS`.

Deleting the constant then leaves zero matches and reds the test.

### 7.3 Integration

`iam-console/tests/integration/dead-letter-commands.test.ts` gains cases for
`bulkReplayDeadLetters`:

- a success, asserting `replayed` reaches the caller as a `number`;
- the request IAM received, asserting `eventType`, `parkedFrom`, `parkedTo` and `maxRows` — the
  timestamps absent for an empty bound and the right instant otherwise, and `maxRows` as a `bigint`;
- a denial, asserting the `forbidden` presentation reaches the caller.

`iam-console/tests/integration/dead-letters-page.test.ts` gains one case proving the canonical bounds
reach IAM on the list call.

### 7.4 The e2e world

Four changes to `ts/apps/iam-console/tests/e2e/support/world.ts`:

1. **`ALL_ACTIONS` gains `'ReplayOutboxDeadLetter'`.** Without it, `authz.isAuthorized` answers
   `false` (fact 14), D4 hides the row Replay buttons, and **R17 reds at
   `tests/e2e/dead-letters.spec.ts:23`**, which clicks Replay. This is the single most likely way to
   mis-read this work as a regression.
2. **`outbox.listDeadLetters` gains the parked-window filter**, inclusive on both ends (fact 8). It
   filters by `eventType` only today (fact 13). Without this the fake would list rows that bulk
   replay excludes, and R20 would measure the fake's own inconsistency.
3. **`outbox.listDeadLetters` becomes cursor-aware**: page 1 answers a non-empty `nextCursor` when
   the filtered set is larger than the page, and page 2 echoes the cursor it received. It answers
   `nextCursor: ''` unconditionally today, so `page.tsx` renders **no paging link at all** and R20's
   paging step has nothing to click.
4. **`outbox.bulkReplayDeadLetters` is added.** It filters the seeded map the same way, takes at most
   `maxRows` rows, removes them, and answers `{ replayed: BigInt(n) }`.

**The third seeded entry goes to R20 only, through `options.overrides`.** Adding it to
`seededDeadLetters()` would change the counts `dead-letters.spec.ts:20,25,38` assert (2, then 1, then
0) and stop the empty state at `:39` from rendering. R17, R18 and R19 keep the two-entry world.

### 7.5 e2e

`tests/e2e/dead-letters.spec.ts` gains **R20**: set a parked-time filter, assert the request the fake
received, follow the paging link and assert the filter survived it (AC 3), then open the bulk form,
confirm, and assert both the result text and the request the fake recorded (AC 4).

The test waits through `tests/e2e/support/hydration.ts`'s `waitForHydration` before the first click,
as `dead-letters.spec.ts:17` does. The bulk confirm step needs JavaScript, exactly as discard does.

---

## 8. Authorization

`ts/packages/paigasus-console-core/src/authorize.ts`: `IAM_ACTIONS` gains
`'ReplayOutboxDeadLetter'`. **Two prose blocks in that file (`:37-40` and `:59-61`) state that the
console asks nothing about replay or discard.** Both are rewritten: it now asks about replay, and
still asks nothing about discard.

`app/(console)/dead-letters/page.tsx` asks the question itself. The layout cannot pass the answer
down, because a layout's `children` are opaque to it, and the memo in `createMayI` keys on the action
(fact 11), so the layout's `ListOutboxDeadLetters` question does not answer this one.

**Placement, stated once.** Revision 1 gave two incompatible instructions. The rule is:

1. the discovery call and the gate run first, exactly as today;
2. `if (gate === 'not-found') notFound();` — **a hidden screen asks no authorization question**;
3. after the gate and after the query parsers, `const [may, clients] = await Promise.all([mayI(), iamClients()]);`
4. then `const [canReplay, data] = await Promise.all([may('ReplayOutboxDeadLetter', ROOT_PRN), loadDeadLettersPage(...)]);`

So the authorization query is never serial with the list read, and it never runs for a 404.

The call shape is `const may = await mayI(); may('Action', ROOT_PRN)` (fact 11). Revision 1 wrote
`mayI('Action', ROOT_PRN)` throughout, which does not exist.

**`canReplay` reaches the row buttons through a prop**, and the path is named here because no file
carries such a flag today: `page.tsx` → `<DeadLetterTable canReplay={canReplay} rows={rows} />` →
`<DeadLetterRowControls canReplay={canReplay} id={row.id} />`, which renders the Replay form only
when it is `true`. Both props **default to `true`**, so the existing unit tests that render those
components directly keep working. `DeadLetterTable` takes only `{ rows }` today
(`dead-letter-table.tsx:21`) and `DeadLetterRowControls` only `{ id }`
(`dead-letters-frame.tsx:131`), and it renders Replay unconditionally at `:145-150`.

This reverses the page's header comment (`page.tsx:3-5`), which states that the page asks discovery
and not `mayI`. **That comment and its reasoning are rewritten, not only the code.** The new comment
states: the page asks discovery for the gate and the authorization query for the replay affordance;
the query is cosmetic and fails open; IAM decides every action; and it records the one extra
`IsAuthorized` call per render, as SMA-629's layout comment recorded its own.

The commands and actions take no authorization question. That rule is unchanged: IAM decides.

---

## 9. Risks

| # | Risk | Position |
|---|---|---|
| R1 | A bulk replay can publish duplicate events past the broker deduplication window, multiplied by the row budget. | Accepted, and named in the confirmation (§ 6.4 point 4). The console cannot detect it. |
| R2 | A cancelled or timed-out bulk RPC leaves an unknown number replayed. | Accepted, and named in the confirmation. Re-running is safe (fact 5). The frame's `unreached` text already tells the operator to reload. |
| R3 | `MAX_BULK_REPLAY_ROWS` duplicates a Rust constant and can drift. | The drift test in § 7.2 fails when it drifts, and it is placed where Moon actually selects it. |
| R4 | The authorization query fails open, so D4's affordance is not a guard. | Recorded in § 2.4. IAM is the guard. |
| R5 | The bulk form acts on rows the operator never saw, because the budget can exceed the page size. | Named in the confirmation (§ 6.4 point 3). |
| R6 | A row with no `parked_at` disappears from a filtered list **and from a bulk replay** (fact 7). | Named in the table caption and in the confirmation (§ 4.6). It is a property of IAM's query, not of the console. |
| R7 | D1's binding is a user-interface convention, not a server-enforced one. A Root caller can post any scope, and a stale tab posts a scope whose evidence is old. | Accepted. Every `OutboxService` RPC is Root-only (`application/dead_letters.rs:175`), so the caller could issue the same RPC directly. D1's real merit is that it stops the operator from typing a second, unseen scope. |
| R8 | A successful bulk replay revalidates. If the revalidated list read answers `forbidden`, `page.tsx` renders `PageError` and the "Replayed 8 events." result is lost. | Accepted, and unchanged from SMA-629: its `refreshesAfterDeadLetterAction` already revalidates on `ok` for single replay, with the same exposure. The `forbidden`-does-not-revalidate rule there covers a forbidden **action**, not a forbidden re-read. |
| R9 | No console-side deadline bounds R2. `createIamClients` sets no `timeoutMs` (`paigasus-console-core/src/iam-clients.ts:52-55`). | Accepted for this issue. The bound is IAM's own. Setting a console deadline is a cross-cutting change for every RPC, not one for this form. |

---

## 10. Out of scope

- Any proto, Rust or HTTP change. Everything this design needs exists (§ 1).
- A progress indicator or a partial-count report for a long bulk replay. The RPC answers once.
- A separate Cedar action for bulk replay. IAM has none (fact 6).
- `DiscardOutboxDeadLetter` in `IAM_ACTIONS` (D4).
- A bulk discard. The issue does not ask for one, and IAM has no such RPC.
- A console-side RPC deadline (R9).

---

## 11. Files

| File | Change |
|---|---|
| `ts/apps/iam-console/lib/paging.ts` | `parseParkedBound`, `PARKED_BOUND_PATTERN`, `MAX_PARKED_BOUND_LENGTH`, `parkedBoundInvalid()` |
| `ts/apps/iam-console/app/(console)/dead-letters/page.tsx` | two parsers, the frame key, the D6 refusal branch, the filter form, the loader call, the paging links, the authorization question, `canReplay`, the header comment |
| `ts/apps/iam-console/app/(console)/dead-letters/load.ts` | `parkedFrom` and `parkedTo` on the list request |
| `ts/apps/iam-console/app/(console)/dead-letters/commands.ts` | `bulkReplayForm`, `parkedBoundField`, `BulkReplayResult`, `bulkReplayDeadLetters` |
| `ts/apps/iam-console/app/(console)/dead-letters/actions.ts` | `bulkReplayDeadLettersAction` |
| `ts/apps/iam-console/app/(console)/dead-letters/dead-letters-frame.tsx` | `runBulk`, the `bulk-answer` arm, `bulkReplay` on `DeadLetterActions`, exported `useRunner`, `canReplay` on `DeadLetterRowControls`, the bulk result text |
| `ts/apps/iam-console/app/(console)/dead-letters/dead-letter-table.tsx` | the `canReplay` prop passed through, and the caption sentence (§ 4.6) |
| `ts/apps/iam-console/app/(console)/dead-letters/bulk-replay-form.tsx` | **new** |
| `ts/packages/paigasus-console-core/src/authorize.ts` | `ReplayOutboxDeadLetter` in `IAM_ACTIONS`, and the two comment blocks |
| `ts/packages/paigasus-console-core/src/form.ts` | `MAX_BULK_REPLAY_ROWS`, re-exported from the index |
| `ts/packages/paigasus-console-core/moon.yml` | `dead_letter.rs` as a `test` input (§ 7.2) |
| `ts/packages/paigasus-console-core/tests/unit/bulk-replay-ceiling.test.ts` | **new** |
| `ts/apps/iam-console/tests/e2e/support/world.ts` | `ALL_ACTIONS`, the window filter, the cursor, the bulk handler (§ 7.4) |
| `ts/apps/iam-console/tests/...` | the eight files of § 7.1, plus § 7.3 and § 7.5 |
| `docs/ops/RUNBOOK-observability.md` | `:421` says "Bulk replay stays API-only." It is no longer true. |

---

## 12. Challenge log (revision 1 → 2)

| Finding | Result | Where |
|---|---|---|
| BLOCKER — `action-names.test.ts:67` asserts `not.toContain('ReplayOutboxDeadLetter')`; `world.ts`'s `ALL_ACTIONS` and `world-actions.test.ts` also red, and a denied replay reds R17 | accepted; revision 1's "no edit to the test" was false | § 7.1, § 7.4 |
| BLOCKER — the drift test is inoperative: `iam-console-ts:test` has no `/rs/**` input, so Moon serves a cached pass | accepted; the constant and the test move to `console-core` | § 6.5, § 7.2 |
| BLOCKER — "`Runner.run` widens" cannot compile: `'bulk-replay'` is not a key of `DeadLetterActions`, the return type splits, and `control` no longer narrows | accepted; a second method `runBulk`, and `DeadLetterControl` does not widen | § 6.1 |
| BLOCKER — D4 hides the row Replay buttons but no file carries the flag | accepted; the `canReplay` prop path is named, defaulting to `true` | § 8, § 11 |
| BLOCKER — `ArchiveButton`'s step 1 has no form, so `max_rows` would sit outside the submitted form; and the confirmation can open with no number, which breaks AC 2 | accepted; one form spans both steps, the input is controlled, and the first button is disabled until the value is valid | § 6.3, § 6.4 |
| MAJOR — § 8's two placement sentences contradict each other, and `mayI` is an accessor, not the question | accepted; one placement, and the call shape corrected everywhere | § 8, fact 11 |
| MAJOR — R20's paging step has no link to click: the fake answers `nextCursor: ''` unconditionally | accepted; the fake's list handler becomes cursor-aware | § 7.4 |
| MAJOR — a third seeded entry reds R17's three count assertions and its empty state | accepted; the third entry goes to R20 through `options.overrides` | § 7.4 |
| MAJOR — `e2e-rows.test.ts` and `actions-structure.test.ts` red and were not listed | accepted | § 7.1 |
| MAJOR — "nothing in `paigasus-console-core` changes" contradicts § 1, § 8 and § 11 | accepted; the claim is narrowed to the three types it is true of | § 5 |
| MAJOR — the strict pattern refuses plausible spellings, and a refusal destroys the page and the other filters | accepted as **D6** (Sven, 2026-09-21) | D6, § 4.1, § 4.2, § 4.3 |
| MAJOR — § 7.2's floor assertion does not stop a vacuous pass; the doc comment at `dead_letter.rs:60-62` holds `MAX_BULK_REPLAY` and `(10_000)` | accepted; line-anchored `pub const` match, exactly one match, and the `_` strip | § 7.2 |
| MAJOR — D1's guarantee is client-side only and was stated as a property | accepted; kept as a justification, added as R7 | § 2.1, R7 |
| MAJOR — "no JavaScript is needed to submit them" contradicts § 6.1 and § 7.5 | accepted | § 6.3 |
| MAJOR — the fake's list handler has no window filter, so R20 could measure the fake's inconsistency | accepted | § 7.4 |
| MINOR — `successText` would answer "Discarded" for a bulk control | accepted, and removed at the root: `DeadLetterControl` does not widen | § 6.1 |
| MINOR — no singular form for the count | accepted | § 6.4, § 6.7 |
| MINOR — `z.coerce.number()` accepts `0x10`, `0b1010`, `0o17`, `+7`, `7.` | accepted; a digits-only regular expression, shared with the confirmation gate | § 6.5 |
| MINOR — line citations drifted by one to eight lines | accepted; every citation re-derived on 2026-09-21 | § 3 |
| MINOR — `RUNBOOK-observability.md:421` says "Bulk replay stays API-only" | accepted | § 11 |
| MINOR — bound inclusivity never stated | accepted | fact 8, § 4.1, § 4.3, § 6.4 |
| MINOR — the NULL blind spot covers bulk replay too, not only the list | accepted | fact 7, § 4.6, R6 |
| MINOR — `dead-letters-frame.test.tsx` builds `DeadLetterActions` by hand | accepted; listed, and the field stays required | § 7.1 |
| MINOR — `maxLength` truncates although the parser promises not to | accepted; named as a typing bound | § 4.1 |
| MINOR — the helper text should use `Field`'s `description` prop | accepted | § 4.3, § 6.3 |
| MINOR — no console deadline bounds R2 | accepted as a recorded risk, not a change | R9, § 10 |
| QUESTION — one more `IsAuthorized` call per render | accepted; recorded in the page comment | § 8 |
| QUESTION — where the form sits | accepted; under the filter, above the table | § 6.2 |
| QUESTION — does `parkedBoundField` accept an absent value | accepted; `preprocess` treats `null`/`undefined`/`''` as "no filter" | § 6.5 |
| QUESTION — should the form be hidden on an empty list | rejected; it renders, and § 6.4 point 3 is the answer. Hiding it adds a branch that only removes a truthful "Replayed 0 events." | § 6.2 |
| QUESTION — a revalidated `forbidden` list read loses the result | accepted as a recorded risk. It is unchanged from SMA-629, whose `ok` path already revalidates | R8, § 6.6 |
