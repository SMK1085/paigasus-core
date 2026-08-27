// SPDX-License-Identifier: Apache-2.0
import { create } from '@bufbuild/protobuf';
import { describe, expect, it } from 'vitest';
import type { Auditable } from './audit.js';
import { ActorSchema } from './generated/paigasus/common/v1/actor_pb.js';
import { AuditMetadataSchema } from './generated/paigasus/common/v1/audit_pb.js';
import { AuditableExampleSchema } from './generated/paigasus/common/v1/auditable_example_pb.js';

// Compile-time identity helper: tsc rejects the call below if the generated
// AuditableExample does not structurally satisfy Auditable. This is the real
// proof of the contract; the runtime assertion just guards against a no-op.
const asAuditable = (a: Auditable): Auditable => a;

describe('Auditable', () => {
  it('the generated AuditableExample structurally satisfies Auditable', () => {
    const prn = 'prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000001';
    const dto = asAuditable(
      create(AuditableExampleSchema, {
        id: 'x',
        audit: create(AuditMetadataSchema, { creator: create(ActorSchema, { prn }) }),
      }),
    );
    expect(dto.audit?.creator?.prn).toBe(prn);
  });

  it('audit is optional', () => {
    const empty: Auditable = {};
    expect(empty.audit).toBeUndefined();
  });
});
