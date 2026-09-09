// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { create, fromBinary, toBinary } from '@bufbuild/protobuf';
import { ErrorInfoSchema } from './generated/google/rpc/error_details_pb.js';

// This file is a fast local signal that the two-invocation codegen ordering in
// contracts/moon.yml has not regressed. It is NOT the control — buf.gen.yaml's
// `clean: true` wipes the shared out: tree, so a reversed order deletes the
// generated module, and what catches that in CI is the codegen-drift step
// (.github/workflows/ci.yml:309-322), which runs unconditionally and reports
// the deletion through `git diff --exit-code`. See spec § 5.1.
describe('generated google.rpc.ErrorInfo', () => {
  it('is generated with its canonical type name', () => {
    expect(ErrorInfoSchema.typeName).toBe('google.rpc.ErrorInfo');
  });

  it('carries the three fields SMA-504 populates', () => {
    const names = ErrorInfoSchema.fields.map((f) => f.name).sort();
    expect(names).toEqual(['domain', 'metadata', 'reason']);
  });

  it('round-trips the (domain, reason, metadata) triple IAM emits', () => {
    const info = create(ErrorInfoSchema, {
      reason: 'slug-conflict',
      domain: 'iam.paigasus.io',
      metadata: { retryable: 'false', correlation_id: 'abc' },
    });
    const decoded = fromBinary(ErrorInfoSchema, toBinary(ErrorInfoSchema, info));
    expect(decoded.reason).toBe('slug-conflict');
    expect(decoded.domain).toBe('iam.paigasus.io');
    expect(decoded.metadata['retryable']).toBe('false');
    expect(decoded.metadata['correlation_id']).toBe('abc');
  });
});
