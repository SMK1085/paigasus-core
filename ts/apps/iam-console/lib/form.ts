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
 * rename path validates a name too since SMA-642, with the same 256 code-point bound, so this
 * schema is no longer the only guard for a rename — it is the one that gives the user the error in
 * the form rather than a round trip. The two trims are not identical: this schema trims as
 * JavaScript does, IAM trims Unicode `White_Space` (SMA-642 spec F2), so a name of only U+0085
 * passes here and IAM answers `invalid-name`.
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
 * bound, because IAM stored renamed names without a check before SMA-642 and those rows are kept as
 * they are (SMA-642 D4). A stricter schema here would refuse every rename of such a node. It
 * carries no `.max()` of its own: this field is only COMPARED (see `renameForm` and `renameChange`)
 * and never sent to IAM, and a Next Server Action request body is already bounded (1 MB by
 * default), which bounds how large it can arrive.
 */
export const currentField = z.string().trim();

export type RenameFields = { readonly slug: string; readonly name: string; readonly currentSlug: string; readonly currentName: string };
export type RenameChange = { newSlug?: string; newName?: string };

/**
 * The shared shape of the three rename forms (SMA-630 CR round 1, spec § 4.2): `prn`, `slug`,
 * `name` and the two hidden "current value" fields. IAM holds names longer than 256 code points
 * that it stored before SMA-642 and does not migrate (SMA-642 D4), so `nameField`'s bound applies
 * ONLY when the trimmed name changed. An unchanged name is accepted as it is: it is never sent to
 * IAM either way (`renameChange` omits an unchanged field). A CHANGED name still must pass
 * `nameField`. Do NOT make it unconditional now that IAM validates: that would refuse every
 * slug-only rename of such a node. This is the ONE place that builds a rename schema; each
 * `commands.ts` calls it instead of repeating the shape.
 */
export function renameForm() {
  return z.object({ prn: prnField, slug: slugField, name: z.string().trim(), currentSlug: currentField, currentName: currentField }).superRefine((value, ctx) => {
    if (value.name.trim() === value.currentName.trim()) return;
    const result = nameField.safeParse(value.name);
    if (!result.success) {
      for (const issue of result.error.issues) ctx.addIssue({ ...issue, path: ['name'] });
    }
  });
}

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
