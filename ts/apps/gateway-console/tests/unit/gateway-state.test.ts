// SPDX-License-Identifier: Apache-2.0
//
// The pure gateway ServiceState reader (spec § 6). It carries the ONE capability rule this zone
// has, so every branch is testable here without a Next request scope.
import { describe, expect, it } from 'vitest';
import type { ServiceState } from '@paigasus/discovery/types';
import { gatewayView } from '../../app/_components/gateway-state';

function available(service: string, capabilities: readonly string[]): ServiceState {
  return { state: 'available', service, descriptor: { service, version: '1.2.3', capabilities }, capabilities };
}

function absent(service: string): ServiceState {
  return { state: 'absent', service };
}

describe('gatewayView', () => {
  it('reports streaming when the gateway is available and lists the capability', () => {
    expect(gatewayView(available('gateway', ['gateway.chat.stream'])).streaming).toBe(true);
  });

  it('does NOT report streaming from a DEGRADED service, even when its last descriptor listed it', () => {
    const state: ServiceState = {
      state: 'degraded',
      service: 'gateway',
      reason: 'timeout',
      descriptor: { service: 'gateway', version: '1.2.3', capabilities: ['gateway.chat.stream'] },
      capabilities: ['gateway.chat.stream'],
    };
    expect(gatewayView(state).streaming).toBe(false);
    expect(gatewayView(state).version).toBe('1.2.3');
    expect(gatewayView(state).reason).toBe('timeout');
  });

  it('reports nothing at all for an absent service', () => {
    expect(gatewayView(absent('gateway'))).toEqual({ state: 'absent', reason: null, version: null, capabilities: [], streaming: false });
  });
});
