// SPDX-License-Identifier: Apache-2.0
//
// The commands behind the dead-letters Server Actions (SMA-629 spec § 6.4). A command takes its IAM
// client as a port, so the tier-2 tests call it against the fake IAM with no session and no Next
// runtime. It takes NO mayI (SMA-511 § 6.3): the page is Root-only, and IAM decides.
import 'server-only';
import { z } from 'zod';
import { callIam, toActionResult, type ActionResult, type IamClients } from '@paigasus/console-core';

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
