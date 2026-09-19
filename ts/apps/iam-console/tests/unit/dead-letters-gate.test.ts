// SPDX-License-Identifier: Apache-2.0
//
// deadLettersGate (SMA-629 spec § 6.2, § 7.2, § 8): every branch, including a degraded IAM whose
// cached descriptor has no key, which is `degraded` and not a 404.
import type { ServiceState } from '@paigasus/discovery/types';
import { describe, expect, it } from 'vitest';
import { deadLettersGate } from '../../app/(console)/dead-letters/load';

const descriptor = (capabilities: string[]) => ({ service: 'iam', version: '1.0.0', capabilities });

describe('deadLettersGate', () => {
  it.each<[string, ServiceState, ReturnType<typeof deadLettersGate>]>([
    ['absent', { state: 'absent', service: 'iam' }, 'not-found'],
    ['available with iam.deadletters', { state: 'available', service: 'iam', descriptor: descriptor(['iam.deadletters']), capabilities: ['iam.deadletters'] }, 'available'],
    ['available without iam.deadletters (an older IAM, AC 2)', { state: 'available', service: 'iam', descriptor: descriptor(['iam.audit']), capabilities: ['iam.audit'] }, 'not-found'],
    ['degraded with the key', { state: 'degraded', service: 'iam', reason: 'timeout', descriptor: descriptor(['iam.deadletters']), capabilities: ['iam.deadletters'] }, 'degraded'],
    [
      'degraded with a cached descriptor that has no key (§ 8)',
      { state: 'degraded', service: 'iam', reason: 'timeout', descriptor: descriptor(['iam.audit']), capabilities: ['iam.audit'] },
      'degraded',
    ],
    ['degraded with no descriptor', { state: 'degraded', service: 'iam', reason: 'network', descriptor: null, capabilities: [] }, 'degraded'],
  ])('%s', (_label, state, gate) => {
    expect(deadLettersGate(state)).toBe(gate);
  });
});
