// SPDX-License-Identifier: Apache-2.0
import { create } from '@bufbuild/protobuf';
import { describe, expect, it } from 'vitest';
import type { Auditable } from './audit.js';
import { AuditMetadataSchema } from './generated/paigasus/common/v1/audit_pb.js';
import { AuditableExampleSchema } from './generated/paigasus/common/v1/auditable_example_pb.js';

// Compile-time identity helper: tsc rejects the call below if the generated
// AuditableExample does not structurally satisfy Auditable. This is the real
// proof of the contract; the runtime assertion just guards against a no-op.
const asAuditable = (a: Auditable): Auditable => a;

describe('Auditable', () => {
  it('the generated AuditableExample structurally satisfies Auditable', () => {
    const dto = asAuditable(
      create(AuditableExampleSchema, {
        id: 'x',
        audit: create(AuditMetadataSchema, { createdBy: 'svc' }),
      }),
    );
    expect(dto.audit?.createdBy).toBe('svc');
  });

  it('audit is optional', () => {
    const empty: Auditable = {};
    expect(empty.audit).toBeUndefined();
  });
});
