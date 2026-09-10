// SPDX-License-Identifier: Apache-2.0
import { ErrorReason, ErrorReasonSchema, asWireReason, fromWireReason } from '@paigasus/proto';
import { describe, expect, it } from 'vitest';

import { presentationFor } from '../src/errors/presentation.js';

const REAL_REASONS = ErrorReasonSchema.values.filter((v) => v.name !== 'ERROR_REASON_UNSPECIFIED');

describe('AC 3 — the override table is total over the registry', () => {
  // The registry is the source of truth, not a number in this file. If a reason is added to
  // error.proto and regenerated, this count moves and the loop below covers the new member.
  it('sees the whole registry', () => {
    expect(REAL_REASONS).toHaveLength(57);
  });

  // This is AC 3 verbatim: driven off the descriptor, so an unmapped code fails a test rather
  // than rendering an empty toast. It is the SECOND mechanism — the total Record type is the
  // first, and it alone can be switched off by a refactor to Partial<...>, which this notices.
  it.each(REAL_REASONS.map((v) => [v.name, v.number] as const))('%s has an entry and round-trips', (_name, number) => {
    const reason = number as ErrorReason;

    // Guarded, NOT `fromWireReason(asWireReason(reason))`: asWireReason returns
    // `string | undefined` and fromWireReason takes `string`, so the nested form does not
    // compile (spec § 9.4).
    const wire = asWireReason(reason);
    expect(wire).toBeDefined();
    expect(fromWireReason(wire!)).toBe(reason);

    expect(presentationFor(reason)).toBeDefined();
  });
});

describe('the entries that change an answer (spec § 9.4)', () => {
  // Load-bearing: capability-disabled arrives only as gRPC Unimplemented, which § 9.2 maps to
  // `generic`. This entry is what turns it into the "switched off" screen.
  it('lifts CAPABILITY_DISABLED to disabled', () => {
    expect(presentationFor(ErrorReason.CAPABILITY_DISABLED)).toBe('disabled');
  });

  // Load-bearing: IAM maps principal-inactive to PermissionDenied/403, so § 9.2 alone would show
  // "you do not have permission" for an account that exists and is switched off.
  it('lifts PRINCIPAL_INACTIVE to disabled', () => {
    expect(presentationFor(ErrorReason.PRINCIPAL_INACTIVE)).toBe('disabled');
  });

  // Its two neighbours keep `forbidden` — they are provisioning states an admin resolves. This is
  // a considered split, not an oversight, so it is asserted.
  it('leaves the two provisioning neighbours on the transport', () => {
    expect(presentationFor(ErrorReason.IDENTITY_NOT_PROVISIONED)).toBe('from-transport');
    expect(presentationFor(ErrorReason.PROVISIONING_FAILED)).toBe('from-transport');
  });

  // A PIN, not a change: 422 (IAM) and 400 (gateway) both already resolve to `invalid-input`.
  // The entry exists so a future status change cannot silently split one code's presentation
  // across two services (error.proto:239-247).
  it('pins INVALID_REQUEST_SCHEMA to invalid-input', () => {
    expect(presentationFor(ErrorReason.INVALID_REQUEST_SCHEMA)).toBe('invalid-input');
  });
});

describe('the sentinel', () => {
  // The Record excludes UNSPECIFIED, but fromWireReason's RETURN TYPE includes it, and no
  // narrowing expresses "not UNSPECIFIED". presentationFor is the one place that handles it, so
  // call sites can pass any ErrorReason.
  it('resolves UNSPECIFIED to from-transport rather than throwing', () => {
    expect(presentationFor(ErrorReason.UNSPECIFIED)).toBe('from-transport');
  });
});
