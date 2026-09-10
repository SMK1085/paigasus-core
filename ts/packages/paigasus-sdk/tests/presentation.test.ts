// SPDX-License-Identifier: Apache-2.0
import { ErrorReason, ErrorReasonSchema, asWireReason, fromWireReason } from '@paigasus/proto';
import { describe, expect, it } from 'vitest';

import { PRESENTATION, presentationOverride } from '../src/errors/presentation.js';

describe('AC 2 — the override table is driven off the registry', () => {
  // The registry's own values, not a hand-written list. An unmapped code fails HERE rather than
  // rendering an empty toast in a console months later.
  const reasons = ErrorReasonSchema.values.filter((v) => v.number !== 0);

  it('sees the whole registry', () => {
    expect(reasons).toHaveLength(57);
  });

  it.each(reasons.map((v) => [v.name, v.number] as const))('%s has an entry and round-trips', (_name, number) => {
    const reason = number;
    const wire = asWireReason(reason);
    expect(wire).toBeDefined();
    expect(fromWireReason(wire as string)).toBe(reason);
    expect(PRESENTATION).toHaveProperty(String(number));
  });
});

describe('the three overrides that change or pin an answer', () => {
  // Spec § 9.4. UPSTREAM_ERROR is the one where the default is visibly WRONG: its arm sets
  // status 200, the HTTP table has no 200 row, so without this entry the mid-stream failure the
  // parser exists to catch would render as `generic`. chat.rs:59-62 documents the opposite.
  it('maps UPSTREAM_ERROR to degraded', () => {
    expect(presentationOverride(ErrorReason.UPSTREAM_ERROR)).toBe('degraded');
  });

  it.each([ErrorReason.UPSTREAM_UNAVAILABLE, ErrorReason.UPSTREAM_TIMEOUT])('pins %i to degraded', (reason) => {
    expect(presentationOverride(reason)).toBe('degraded');
  });

  // IAM answers 422 and the gateway answers 400 for this identical wire code
  // (error.proto:239-247). Both already reach `invalid-input` from the status table, so this
  // entry PINS that agreement: neither status can later split the presentation.
  it('pins INVALID_REQUEST_SCHEMA to invalid-input', () => {
    expect(presentationOverride(ErrorReason.INVALID_REQUEST_SCHEMA)).toBe('invalid-input');
  });

  // gRPC Unimplemented with no HTTP form at all (convert.rs:96-104). Unimplemented reads as
  // "this build cannot do that"; the product meaning is "this deployment turned it off".
  it('maps CAPABILITY_DISABLED to disabled', () => {
    expect(presentationOverride(ErrorReason.CAPABILITY_DISABLED)).toBe('disabled');
  });
});

describe('every other reason defers to the transport', () => {
  it('returns null for a from-transport reason', () => {
    expect(presentationOverride(ErrorReason.SLUG_CONFLICT)).toBeNull();
  });

  it('returns null for an unmapped reason', () => {
    expect(presentationOverride(null)).toBeNull();
  });

  // The sentinel is not a real reason and is not in the table. It must not throw.
  it('returns null for the zero sentinel', () => {
    expect(presentationOverride(ErrorReason.UNSPECIFIED)).toBeNull();
  });
});
