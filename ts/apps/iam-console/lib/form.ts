// SPDX-License-Identifier: Apache-2.0
//
// Shared pieces of the Server Action shells (spec § 5.3). zod checks the SHAPE of a form only
// (present, trimmed, bounded). IAM owns every business rule — the slug grammar, the PRN grammar,
// who may act — and answers with a reason the form copy knows (spec § 6.5).
import 'server-only';
import { z } from 'zod';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { neverReachedIam, type ActionState, type IamResult } from '@paigasus/console-core';

// The field bounds of every tenancy form (SMA-630 spec § 4.2). They live ONLY here: each
// commands.ts imports them, so a create form and a rename form cannot disagree.

/** IAM's NAME_MAX_CHARS (paigasus-iam-core tenancy.rs): 256 Unicode scalar values, not UTF-16 units. */
export const NAME_MAX_CODE_POINTS = 256;

/** A slug. IAM refuses more than 64 bytes with `invalid-slug`; this bound only limits the request. */
export const slugField = z.string().trim().min(1).max(200);

/**
 * A name, for create AND rename. The bound counts code points (`[...value].length`), as IAM does:
 * zod's `.max()` counts UTF-16 units, and would refuse a valid name of astral characters. IAM's
 * rename path does not validate a name (spec F11), so for a rename this schema is the only guard.
 */
export const nameField = z
  .string()
  .trim()
  .min(1)
  .refine((value) => [...value].length <= NAME_MAX_CODE_POINTS);

/** A PRN: bounded text. IAM parses it and answers `invalid-prn` for a bad one. */
export const prnField = z.string().trim().min(1).max(512);

/**
 * A hidden "current value" of a rename form. It can be empty, and it can be longer than the name
 * bound, because IAM stores a renamed name without a check (spec F11). A stricter schema here would
 * refuse every rename of such a node. The bound only limits the request.
 */
export const currentField = z.string().trim().max(4096);

export type RenameFields = { readonly slug: string; readonly name: string; readonly currentSlug: string; readonly currentName: string };
export type RenameChange = { newSlug?: string; newName?: string };

/**
 * The fields a rename sends (SMA-630 spec D6, § 4.3): only the ones the user changed, compared on
 * trimmed values. With no change the result is `{}`. The command then still calls IAM with neither
 * field, and IAM answers `nothing-to-rename`: the console does not decide that refusal itself.
 *
 * This prevents one lost update: user A changes only the slug while user B changes only the name.
 * Two changes of the SAME field still end with the last write (spec § 11).
 */
export function renameChange(fields: RenameFields): RenameChange {
  const slug = fields.slug.trim();
  const name = fields.name.trim();
  return {
    ...(slug === fields.currentSlug.trim() ? {} : { newSlug: slug }),
    ...(name === fields.currentName.trim() ? {} : { newName: name }),
  };
}

/** What a command returns. `null` is only the initial state of `useActionState`. */
export type ActionResult = Exclude<ActionState, null>;

export function toActionResult(result: IamResult<unknown>): ActionResult {
  return result.ok ? { ok: true } : { ok: false, error: result.error };
}

export function formFields(form: FormData, names: readonly string[]): Record<string, FormDataEntryValue | null> {
  return Object.fromEntries(names.map((name) => [name, form.get(name)]));
}

/**
 * A form that zod refused never reached IAM (see `@paigasus/console-core`'s `neverReachedIam`).
 * `transport` says HTTP 400 because the BFF itself refused the request; the field is for logging
 * only, and nothing branches on it (ADR-0019 E8).
 */
export function invalidFormInput(): PaigasusError {
  return neverReachedIam({ presentation: 'invalid-input', message: 'Fill in every field of the form.', transport: { kind: 'http', status: 400 } });
}

/**
 * Whether a rename, archive or restore action refreshes the tenancy pages (SMA-630 spec § 4.4): on a
 * success, as every action does, and ALSO on `forbidden` and `conflict`. For these actions a refusal
 * often means that the page is stale (another user archived the node or took the slug). Without the
 * refresh the page shows "Active" next to a 403. Other refusals change nothing on the page. The five
 * create and membership actions keep their success-only rule.
 */
export function refreshesAfterLifecycleAction(result: ActionResult): boolean {
  return result.ok || result.error.presentation === 'forbidden' || result.error.presentation === 'conflict';
}
