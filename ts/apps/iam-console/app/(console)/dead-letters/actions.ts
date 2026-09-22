// SPDX-License-Identifier: Apache-2.0
'use server';

// The three dead-letter Server Actions (SMA-629 spec § 6.4; SMA-661 spec § 6.6). See
// ../orgs/actions.ts for the rules every action follows; tests/unit/actions-structure.test.ts holds
// them. Each gets its client through iamClientsForAction(), parses the form with zod, and never
// navigates. The frame of the page calls them directly, with `null` as the previous state. They
// refresh the page on ok and on not-found only (lib/form.ts's refreshesAfterDeadLetterAction says why
// forbidden does not refresh).
import { revalidatePath } from 'next/cache';
import { formFields, invalidFormInput, type ActionState } from '@paigasus/console-core';
import { iamClientsForAction } from '../../../lib/console';
import { refreshesAfterDeadLetterAction } from '../../../lib/form';
import { bulkReplayDeadLetters, bulkReplayForm, deadLetterForm, discardDeadLetter, replayDeadLetter, type BulkReplayState } from './commands';

/** basePath-RELATIVE, like TENANCY_PATH: Next adds /iam. Not exported: a 'use server' file exports only async functions. */
const DEAD_LETTERS_PATH = '/dead-letters';

export async function replayDeadLetterAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = deadLetterForm.safeParse(formFields(form, ['id']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await replayDeadLetter({ outbox: clients.value.outbox }, parsed.data);
  if (refreshesAfterDeadLetterAction(result)) revalidatePath(DEAD_LETTERS_PATH, 'page');
  return result;
}

export async function discardDeadLetterAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = deadLetterForm.safeParse(formFields(form, ['id']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await discardDeadLetter({ outbox: clients.value.outbox }, parsed.data);
  if (refreshesAfterDeadLetterAction(result)) revalidatePath(DEAD_LETTERS_PATH, 'page');
  return result;
}

/**
 * Bulk replay (SMA-661 spec § 6.6). Its state carries the replayed count, so it is not an
 * ActionState. A form with no valid `maxRows` never reaches IAM (AC 1). Bulk replay never answers
 * not-found, so in practice it refreshes the page on ok only, and a zero count refreshes too.
 */
export async function bulkReplayDeadLettersAction(_previous: BulkReplayState, form: FormData): Promise<BulkReplayState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = bulkReplayForm.safeParse(formFields(form, ['eventType', 'parkedFrom', 'parkedTo', 'maxRows']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await bulkReplayDeadLetters({ outbox: clients.value.outbox }, parsed.data);
  if (refreshesAfterDeadLetterAction(result)) revalidatePath(DEAD_LETTERS_PATH, 'page');
  return result;
}
