// SPDX-License-Identifier: Apache-2.0
//
// Shared pieces of the Server Action shells (spec § 5.3). zod checks the SHAPE of a form only
// (present, trimmed, bounded). IAM owns every business rule — the slug grammar, the PRN grammar,
// who may act — and answers with a reason the form copy knows (spec § 6.5).
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import type { ActionState, IamResult } from './errors';

/** What a command returns. `null` is only the initial state of `useActionState`. */
export type ActionResult = Exclude<ActionState, null>;

export function toActionResult(result: IamResult<unknown>): ActionResult {
  return result.ok ? { ok: true } : { ok: false, error: result.error };
}

export function formFields(form: FormData, names: readonly string[]): Record<string, FormDataEntryValue | null> {
  return Object.fromEntries(names.map((name) => [name, form.get(name)]));
}

/**
 * A form that zod refused never reached IAM, so this error carries no IAM data: no domain, no
 * reason, no correlation id. `transport` says HTTP 400 because the BFF itself refused the request;
 * the field is for logging only, and nothing branches on it (ADR-0019 E8).
 */
export function invalidFormInput(): PaigasusError {
  return {
    presentation: 'invalid-input',
    domain: null,
    reason: null,
    rawReason: null,
    rawDomain: null,
    message: 'Fill in every field of the form.',
    correlationId: null,
    requestId: null,
    retryable: false,
    metadata: {},
    transport: { kind: 'http', status: 400 },
  };
}

/**
 * The session ended before the user submitted the form. Like `invalidFormInput`, this error never
 * reached IAM, so it carries no IAM data.
 *
 * `presentation: 'relogin'` makes `FormError` render the `SignInAgain` LINK (spec § 6.4: relogin is
 * a link, never an automatic redirect). That link is what keeps the browser inside the zone — see
 * `lib/iam.ts`'s `iamClientsForAction` for why a Server Action must not redirect here.
 */
export function sessionExpired(): PaigasusError {
  return {
    presentation: 'relogin',
    domain: null,
    reason: null,
    rawReason: null,
    rawDomain: null,
    message: 'The session has ended.',
    correlationId: null,
    requestId: null,
    retryable: false,
    metadata: {},
    transport: { kind: 'http', status: 401 },
  };
}
