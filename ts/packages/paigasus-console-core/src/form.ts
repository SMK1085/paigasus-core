// SPDX-License-Identifier: Apache-2.0
//
// The PURE form helpers of the Server Action shells, shared by both console zones (SMA-636 D11,
// spec § 4.5). They moved here from iam-console's lib/form.ts. zod checks the SHAPE of a form only
// (present, trimmed, bounded). IAM owns every business rule — the name rules, the PRN grammar, who
// may act — and answers with a reason the form copy knows.
//
// The rename helpers (renameForm, renameChange, slugField, currentField,
// refreshesAfterLifecycleAction) stay in iam-console: only that zone renames a node.
//
// A client component imports `FormAction` and `ActionResult` with `import type`, which
// verbatimModuleSyntax erases, so this server-only module never reaches a client bundle.
import 'server-only';
import { z } from 'zod';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { neverReachedIam, type ActionState, type IamResult } from './errors';

/** IAM's NAME_MAX_CHARS (paigasus-iam-core tenancy.rs): 256 Unicode scalar values, not UTF-16 units. */
export const NAME_MAX_CODE_POINTS = 256;

/**
 * The largest bulk-replay budget the console sends (SMA-661 spec § 6.5, D3). IAM clamps `max_rows`
 * to BulkReplayRequest::MAX_BULK_REPLAY (paigasus-iam-core dead_letter.rs) and says nothing, so a
 * larger budget would make the confirmation name a number that IAM never replays. The console
 * refuses it instead. tests/unit/bulk-replay-ceiling.test.ts holds this value to the Rust constant.
 */
export const MAX_BULK_REPLAY_ROWS = 10_000;

/**
 * A name: of a tenancy node, and of a service account (IAM trims it and allows 1–256 characters,
 * SMA-636 spec § 3.1). The bound counts code points (`[...value].length`), as IAM does. The two
 * trims are not identical: this schema trims as JavaScript does, IAM trims Unicode `White_Space`
 * (SMA-642 spec F2), so a name of only U+0085 passes here and IAM answers `invalid-name`.
 */
export const nameField = z
  .string()
  .trim()
  .min(1)
  .refine((value) => [...value].length <= NAME_MAX_CODE_POINTS);

/** A PRN: bounded text. IAM parses it and answers `invalid-prn` for a bad one. */
export const prnField = z.string().trim().min(1).max(512);

/** What a command returns. `null` is only the initial state of a form. */
export type ActionResult = Exclude<ActionState, null>;

export function toActionResult(result: IamResult<unknown>): ActionResult {
  return result.ok ? { ok: true } : { ok: false, error: result.error };
}

/**
 * The named fields of a form, and NOTHING else (SMA-636 § 5.1, "accepted inputs"). A field the
 * action does not name never reaches its zod shape, whatever the client added to the form.
 */
export function formFields(form: FormData, names: readonly string[]): Record<string, ReturnType<FormData['get']>> {
  return Object.fromEntries(names.map((name) => [name, form.get(name)]));
}

/**
 * A form that zod refused never reached IAM (see `neverReachedIam`). `transport` says HTTP 400
 * because the BFF itself refused the request; the field is for logging only, and nothing branches
 * on it (ADR-0019 E8).
 */
export function invalidFormInput(): PaigasusError {
  return neverReachedIam({ presentation: 'invalid-input', message: 'Fill in every field of the form.', transport: { kind: 'http', status: 400 } });
}

/** The shape of a Server Action that a client form posts to. */
export type FormAction = (previous: ActionState, form: FormData) => Promise<ActionState>;
