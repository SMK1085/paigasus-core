// SPDX-License-Identifier: Apache-2.0
import { create } from '@bufbuild/protobuf';
import { describe, expect, it } from 'vitest';
import { CheckResponseSchema } from './generated/paigasus/gateway/v1/health_pb.js';

describe('generated HealthService types', () => {
  it('CheckResponse carries status', () => {
    const resp = create(CheckResponseSchema, { status: 'ok' });
    expect(resp.status).toBe('ok');
  });
});
