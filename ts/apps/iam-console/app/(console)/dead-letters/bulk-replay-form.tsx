// SPDX-License-Identifier: Apache-2.0
//
// The bulk-replay form of /iam/dead-letters (SMA-661 spec § 6.2–§ 6.4). CLIENT component. It sits
// under the filter form, and it is BOUND to the filter (D1): it shows the scope as read-only text and
// posts it as three hidden fields, so the operator cannot type a second, unseen scope. The operator
// types only the row budget. The binding is a user-interface convention, not a server-enforced
// property (R7): a Root caller can post any scope.
//
// ONE <form> wraps both steps, unlike ArchiveButton (which renders no form in step 1): the budget
// and the hidden scope must be inside the element that submits. The budget is CONTROLLED state,
// because the confirmation names it (AC 2), and the first button stays disabled until it is a whole
// number from 1 to the ceiling. The form hands its FormData to the frame's runner, as every row
// control does; it never uses useActionState, because the frame owns the only result region.
'use client';

import { useState, type FormEvent, type ReactElement } from 'react';
import { Field, Input, PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS } from '@paigasus/ui';
import { useRunner } from './dead-letters-frame';
import { MAX_ROWS_PATTERN } from './max-rows';

/** The scope of a bulk replay: the list filter as CANONICAL values. '' is no filter. */
export type BulkReplayScope = { readonly eventType: string; readonly parkedFrom: string; readonly parkedTo: string };

/** § 6.4 point 3. `50` is lib/paging.ts's PAGE_SIZE, which a client module cannot import; a unit test holds the two equal. */
const BEYOND_THE_PAGE = 'The scope can hold events this page does not show. A page holds 50 events, and the budget can be larger.';

/** § 6.4 point 4: the two risks (R1, R2). */
const BULK_RISKS =
  'A replay can publish an event a second time when the broker deduplication window has passed. A cancelled or timed-out request can leave an unknown number of events already replayed; running it again is safe, because a replayed event is no longer parked.';

/** § 6.4 point 5 (fact 7): a row with no parked_at never satisfies a time bound. */
const NO_PARKED_TIME = 'An event with no parked time is not in this scope.';

/** § 6.4 point 2 and D5: an empty scope is allowed, and it is named in words. */
const EVERY_PARKED_EVENT = 'The scope is EVERY parked event: no event type and no parked-time window.';

function eventWords(eventType: string): string {
  return eventType === '' ? 'any event type' : `event type ${eventType}`;
}

/** The window in words. Both bounds are inclusive (fact 8); an absent bound is named as absent. */
function windowWords(scope: BulkReplayScope): string {
  if (scope.parkedFrom !== '' && scope.parkedTo !== '') return `parked from ${scope.parkedFrom} to ${scope.parkedTo}, both bounds included`;
  if (scope.parkedFrom !== '') return `parked from ${scope.parkedFrom}, the bound included, with no end`;
  if (scope.parkedTo !== '') return `parked up to ${scope.parkedTo}, the bound included, with no start`;
  return 'any parked time';
}

/** § 6.3: the read-only scope line above the budget. */
export function bulkReplayScopeText(scope: BulkReplayScope): string {
  if (scope.parkedFrom === '' && scope.parkedTo === '') return `Scope: ${eventWords(scope.eventType)}, any parked time`;
  return `Scope: ${eventWords(scope.eventType)}, parked ${scope.parkedFrom === '' ? '(no start)' : scope.parkedFrom} → ${scope.parkedTo === '' ? '(no end)' : scope.parkedTo}`;
}

/**
 * § 6.4: the confirmation. It names the budget EXACTLY (AC 2): the form refused every value IAM would
 * clamp. Then the scope in words, that the scope reaches past the page, the two risks, and the NULL
 * blind spot when either bound is set.
 */
export function bulkReplayConfirmation(input: BulkReplayScope & { readonly maxRows: number }): string {
  const windowed = input.parkedFrom !== '' || input.parkedTo !== '';
  const budget = `Replay up to ${String(input.maxRows)} parked ${input.maxRows === 1 ? 'event' : 'events'}.`;
  const scope = input.eventType === '' && !windowed ? EVERY_PARKED_EVENT : `Scope: ${eventWords(input.eventType)}, ${windowWords(input)}.`;
  return [budget, scope, BEYOND_THE_PAGE, BULK_RISKS, ...(windowed ? [NO_PARKED_TIME] : [])].join(' ');
}

/** The budget the operator typed, or null while the first button must stay disabled (§ 6.4). */
export function maxRowsOf(raw: string, ceiling: number): number | null {
  const value = raw.trim();
  if (!MAX_ROWS_PATTERN.test(value)) return null;
  const rows = Number(value);
  return rows >= 1 && rows <= ceiling ? rows : null;
}

/** `ceiling` is MAX_BULK_REPLAY_ROWS: the page passes it, because @paigasus/console-core is server-only. */
export function BulkReplayForm({ scope, ceiling }: { readonly scope: BulkReplayScope; readonly ceiling: number }): ReactElement {
  const runner = useRunner();
  const [maxRows, setMaxRows] = useState('');
  const [confirming, setConfirming] = useState(false);
  const rows = maxRowsOf(maxRows, ceiling);

  function submit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    // Enter in the budget input submits a form that has one text input and no submit button, even in
    // step 1. Only the confirm button of step 2 may run the action.
    if (!confirming) return;
    runner.runBulk(new FormData(event.currentTarget));
    setConfirming(false);
  }

  return (
    <section aria-labelledby="dead-letters-bulk-heading" className="rounded-pgs flex flex-col gap-2 border p-4">
      <h2 id="dead-letters-bulk-heading" className="text-lg font-semibold">
        Bulk replay
      </h2>
      <p className="text-sm">{bulkReplayScopeText(scope)}</p>
      <form aria-label="Bulk replay" className="flex flex-col gap-2" onSubmit={submit}>
        <input type="hidden" name="eventType" value={scope.eventType} />
        <input type="hidden" name="parkedFrom" value={scope.parkedFrom} />
        <input type="hidden" name="parkedTo" value={scope.parkedTo} />
        <Field label="Max rows" htmlFor="dead-letters-max-rows" description={`Enter a whole number from 1 to ${String(ceiling)}.`}>
          <Input
            required
            name="maxRows"
            value={maxRows}
            inputMode="numeric"
            autoComplete="off"
            readOnly={confirming}
            onChange={(event) => {
              setMaxRows(event.target.value);
            }}
          />
        </Field>
        {confirming && rows !== null ? (
          <div className="flex flex-col gap-2">
            <p className="text-sm">{bulkReplayConfirmation({ ...scope, maxRows: rows })}</p>
            <div className="flex flex-wrap gap-2">
              <button type="submit" disabled={runner.busy} className={PRIMARY_BUTTON_CLASS}>
                Confirm bulk replay
              </button>
              <button
                type="button"
                disabled={runner.busy}
                className={SECONDARY_BUTTON_CLASS}
                onClick={() => {
                  setConfirming(false);
                }}
              >
                Cancel
              </button>
            </div>
          </div>
        ) : (
          <div>
            <button
              type="button"
              disabled={runner.busy || rows === null}
              className={SECONDARY_BUTTON_CLASS}
              onClick={() => {
                setConfirming(true);
              }}
            >
              Bulk replay…
            </button>
          </div>
        )}
      </form>
    </section>
  );
}
