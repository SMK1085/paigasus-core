// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { asWireDomain, asWireReason, fromWireDomain, fromWireReason } from './error.js';
import {
  ErrorDomain,
  ErrorReason,
  ErrorReasonSchema,
} from './generated/paigasus/common/v1/error_pb.js';

describe('asWireReason', () => {
  it('spells the registry codes exactly', () => {
    expect(asWireReason(ErrorReason.SLUG_CONFLICT)).toBe('slug-conflict');
    expect(asWireReason(ErrorReason.INVALID_REQUEST_SCHEMA)).toBe('invalid-request-schema');
    expect(asWireReason(ErrorReason.CAPABILITY_DISABLED)).toBe('capability-disabled');
    expect(asWireReason(ErrorReason.INTERNAL)).toBe('internal');
  });

  it('has no wire spelling for the zero sentinel', () => {
    expect(asWireReason(ErrorReason.UNSPECIFIED)).toBeUndefined();
  });

  it('has no wire spelling for a number the enum does not know', () => {
    // A newer service can emit a code this build's generated enum predates.
    expect(asWireReason(99999 as ErrorReason)).toBeUndefined();
  });
});

describe('fromWireReason', () => {
  it('resolves every code this build declares', () => {
    expect(fromWireReason('slug-conflict')).toBe(ErrorReason.SLUG_CONFLICT);
    expect(fromWireReason('authn-unavailable')).toBe(ErrorReason.AUTHN_UNAVAILABLE);
  });

  it('has no value for the zero sentinel spelling', () => {
    expect(fromWireReason('unspecified')).toBeUndefined();
  });

  it('rejects malformed input rather than folding it', () => {
    // Parity with the Rust from_wire_reason_rejects_malformed_input test
    // (rs/crates/libs/paigasus-proto/src/error.rs:287-302). The list is
    // identical, deliberately: a laxer TypeScript parser would accept codes the
    // service can never emit, and would weaken the consumed-side table test in
    // SMA-625.
    //
    // The last two are the reason validation runs BEFORE any case transform,
    // not after. MEASURED on Node 24.16.0: "ınternal".toUpperCase() is exactly
    // "INTERNAL" and "ſlug-conflict".toUpperCase() is "SLUG-CONFLICT", because
    // JavaScript folds U+0131 and U+017F just as Rust's str::to_uppercase does.
    // A deny-list check applied after uppercasing would resolve both to real
    // registry values.
    for (const bad of [
      'slug_conflict',
      'SLUG-CONFLICT',
      'Slug-Conflict',
      '',
      '-slug',
      'slug-',
      'slug--conflict',
      'no-such-code',
      'ınternal',
      'ſlug-conflict',
    ]) {
      expect(fromWireReason(bad), bad).toBeUndefined();
    }
  });
});

describe('the reason codec round-trips the whole registry', () => {
  it('covers every non-sentinel value declared in error.proto', () => {
    const values = ErrorReasonSchema.values.filter((v) => v.name !== 'ERROR_REASON_UNSPECIFIED');

    // Cardinality guard. The Rust mirror asserts 57 at
    // rs/crates/libs/paigasus-proto/src/error.rs:230; the two must agree,
    // because both derive from the same proto.
    expect(values).toHaveLength(57);

    for (const value of values) {
      const reason = value.number as ErrorReason;
      const wire = asWireReason(reason);
      expect(wire, value.name).toBeDefined();
      expect(wire).toMatch(/^[a-z][a-z0-9]*(-[a-z0-9]+)*$/);
      expect(fromWireReason(wire as string)).toBe(reason);
    }
  });
});

describe('the domain codec', () => {
  it('spells both domains with the suffix', () => {
    expect(asWireDomain(ErrorDomain.IAM)).toBe('iam.paigasus.io');
    expect(asWireDomain(ErrorDomain.GATEWAY)).toBe('gateway.paigasus.io');
  });

  it('has no wire spelling for the zero sentinel', () => {
    expect(asWireDomain(ErrorDomain.UNSPECIFIED)).toBeUndefined();
  });

  it('round-trips both domains', () => {
    expect(fromWireDomain('iam.paigasus.io')).toBe(ErrorDomain.IAM);
    expect(fromWireDomain('gateway.paigasus.io')).toBe(ErrorDomain.GATEWAY);
  });

  it('requires the suffix and rejects a malformed label', () => {
    for (const bad of ['iam', 'iam.example.com', 'IAM.paigasus.io', '.paigasus.io', 'ıam.paigasus.io']) {
      expect(fromWireDomain(bad), bad).toBeUndefined();
    }
  });
});
