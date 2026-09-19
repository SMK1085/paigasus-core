// SPDX-License-Identifier: Apache-2.0
//
// The zone overview's full discovery-state matrix (spec § 10.2). Assert on the rendered markup,
// never on the props passed in.
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type { ServiceState } from '@paigasus/discovery/types';
import { gatewayView } from '../../app/_components/gateway-state';
import { ZoneOverview } from '../../app/_components/zone-overview';

function available(capabilities: readonly string[]): ServiceState {
  return { state: 'available', service: 'gateway', descriptor: { service: 'gateway', version: '1.2.3', capabilities }, capabilities };
}

function degraded(): ServiceState {
  return {
    state: 'degraded',
    service: 'gateway',
    reason: 'timeout',
    descriptor: { service: 'gateway', version: '1.2.3', capabilities: ['gateway.chat.stream'] },
    capabilities: ['gateway.chat.stream'],
  };
}

function absent(): ServiceState {
  return { state: 'absent', service: 'gateway' };
}

function render(state: ServiceState): string {
  return renderToStaticMarkup(<ZoneOverview view={gatewayView(state)} />);
}

describe('ZoneOverview', () => {
  it('reports an available gateway with the capability as streaming, with its version', () => {
    const html = render(available(['gateway.chat.stream']));
    expect(html).toContain('data-state="available"');
    expect(html).toContain('data-streaming="true"');
    expect(html).toContain('gateway.chat.stream');
    expect(html).toContain('1.2.3');
  });

  it('reports an available gateway without the capability as not streaming', () => {
    const html = render(available([]));
    expect(html).toContain('data-streaming="false"');
    expect(html).toContain('Streaming chat (gateway.chat.stream) is not available.');
  });

  it('shows a visible reason for a degraded gateway, and does not report streaming', () => {
    const html = render(degraded());
    expect(html).toContain('data-state="degraded"');
    expect(html).toContain('data-testid="gateway-reason"');
    expect(html).toContain('timeout');
    expect(html).toContain('data-streaming="false"');
  });

  it('shows no version and no capability list for an absent gateway', () => {
    const html = render(absent());
    expect(html).toContain('data-state="absent"');
    expect(html).not.toContain('data-testid="gateway-version"');
    expect(html).not.toContain('data-testid="gateway-capabilities"');
  });

  it('always renders "All organizations"', () => {
    expect(render(absent())).toContain('All organizations');
  });
});
