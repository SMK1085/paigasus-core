// SPDX-License-Identifier: Apache-2.0
//
// The commands behind the dead-letters Server Actions (SMA-629 spec § 6.4; SMA-661 spec § 5, § 6.5,
// § 6.6). A command takes its IAM client as a port, so the tier-2 tests call it against the fake IAM
// with no session and no Next runtime. It takes NO mayI (SMA-511 § 6.3): the page is Root-only, and
// IAM decides.
import 'server-only';
import { z } from 'zod';
import { callIam, MAX_BULK_REPLAY_ROWS, toActionResult, type ActionResult, type IamClients } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { canonicalParkedBound, MAX_EVENT_TYPE_LENGTH } from '../../../lib/paging';
import { parkedWindow } from './load';
import { MAX_ROWS_PATTERN } from './max-rows';

/**
 * The id of a dead letter, after a trim. zod 4's `z.uuid()` is RFC-strict and refuses some ids that
 * IAM's `Uuid::parse_str` accepts. That is acceptable: IAM mints UUIDv7 ids, which are RFC 4122 ids.
 */
export const deadLetterForm = z.object({ id: z.string().trim().pipe(z.uuid()) });
export type DeadLetterInput = z.infer<typeof deadLetterForm>;

/** A replay of an id that is no longer parked answers `not-found`. */
export async function replayDeadLetter(deps: { readonly outbox: Pick<IamClients['outbox'], 'replayDeadLetter'> }, input: DeadLetterInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.outbox.replayDeadLetter({ id: input.id })));
}

/** A discard deletes the entry; IAM's audit log keeps a copy of the event. */
export async function discardDeadLetter(deps: { readonly outbox: Pick<IamClients['outbox'], 'discardDeadLetter'> }, input: DeadLetterInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.outbox.discardDeadLetter({ id: input.id })));
}

/**
 * One parked-time bound of the bulk form, a hidden field (SMA-661 spec § 6.5). `''`, `null` and
 * `undefined` all mean "no filter": `formFields` answers `null` for a field that a hand-built POST
 * left out, and an omitted optional filter is not a missing required field. Any other value must pass
 * lib/paging.ts's `canonicalParkedBound`, the SAME check as the GET filter, and comes out canonical.
 */
export const parkedBoundField = z.preprocess(
  (value) => (value === null || value === undefined ? '' : value),
  z
    .string()
    .trim()
    .transform((value, ctx) => {
      if (value === '') return '';
      const iso = canonicalParkedBound(value);
      if (iso === null) {
        ctx.addIssue({ code: 'custom', message: 'not a valid parked time' });
        return z.NEVER;
      }
      return iso;
    }),
);

/**
 * The bulk-replay form (SMA-661 spec § 6.5). `maxRows` is REQUIRED, and it is IAM's only guard on
 * blast radius: an empty or absent value fails the digits pattern, so the action never builds a
 * request (AC 1). The pattern is max-rows.ts's, the one the form's first button is gated on, so the
 * number IAM receives is the number the confirmation named. A value above MAX_BULK_REPLAY_ROWS is
 * refused, because IAM would clamp it without a word (D3).
 */
export const bulkReplayForm = z.object({
  eventType: z.string().trim().max(MAX_EVENT_TYPE_LENGTH),
  parkedFrom: parkedBoundField,
  parkedTo: parkedBoundField,
  maxRows: z.string().trim().regex(MAX_ROWS_PATTERN).transform(Number).pipe(z.number().int().min(1).max(MAX_BULK_REPLAY_ROWS)),
});
export type BulkReplayInput = z.infer<typeof bulkReplayForm>;

/**
 * What a bulk replay answers (SMA-661 spec § 5). ActionResult carries no payload, so the count needs
 * its own type. A value of it is still assignable to ActionResult, so refreshesAfterDeadLetterAction
 * takes it unchanged. `replayed` is a number: IAM's uint64 is at most 10000 (the clamp), so the
 * conversion is exact and no bigint crosses the Server Action boundary.
 */
export type BulkReplayResult = { readonly ok: true; readonly replayed: number } | { readonly ok: false; readonly error: PaigasusError };
export type BulkReplayState = BulkReplayResult | null;
export type BulkReplayAction = (previous: BulkReplayState, form: FormData) => Promise<BulkReplayState>;

/** Bulk replay is NOT atomic (iam.proto:696-699): a failed call can still have replayed some rows. */
export async function bulkReplayDeadLetters(deps: { readonly outbox: Pick<IamClients['outbox'], 'bulkReplayDeadLetters'> }, input: BulkReplayInput): Promise<BulkReplayResult> {
  const result = await callIam(() => deps.outbox.bulkReplayDeadLetters({ eventType: input.eventType, maxRows: BigInt(input.maxRows), ...parkedWindow(input) }));
  return result.ok ? { ok: true, replayed: Number(result.value.replayed) } : { ok: false, error: result.error };
}
