// SPDX-License-Identifier: Apache-2.0
import type { ServiceState } from '@paigasus/discovery/types';
import { describe, expect, it } from 'vitest';
import { auditGate } from '../../app/(console)/audit/load';

const descriptor = (capabilities: string[]) => ({ service: 'iam', version: '1.0.0', capabilities });

describe('auditGate (spec § 6.6)', () => {
  it.each<[string, ServiceState, ReturnType<typeof auditGate>]>([
    ['absent', { state: 'absent', service: 'iam' }, 'not-found'],
    ['available without iam.audit', { state: 'available', service: 'iam', descriptor: descriptor(['iam.authz.cedar']), capabilities: ['iam.authz.cedar'] }, 'not-found'],
    ['available with iam.audit', { state: 'available', service: 'iam', descriptor: descriptor(['iam.audit']), capabilities: ['iam.audit'] }, 'available'],
    ['degraded with the last known capability', { state: 'degraded', service: 'iam', reason: 'timeout', descriptor: descriptor(['iam.audit']), capabilities: ['iam.audit'] }, 'degraded'],
    ['degraded with no descriptor', { state: 'degraded', service: 'iam', reason: 'network', descriptor: null, capabilities: [] }, 'degraded'],
  ])('%s', (_label, state, gate) => {
    expect(auditGate(state)).toBe(gate);
  });
});
