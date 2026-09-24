// SPDX-License-Identifier: Apache-2.0
//
// The composer rule (SMA-635 spec § 6.1).
import { describe, expect, it } from 'vitest';
import type { GatewayView } from '../../app/_components/gateway-state';
import { GATEWAY_UNAVAILABLE_TEXT, STREAMING_OFF_TEXT, composerNotice } from '../../app/_components/playground-notice';

const view = (state: GatewayView['state'], streaming: boolean): GatewayView => ({ state, reason: null, version: null, capabilities: streaming ? ['gateway.chat.stream'] : [], streaming });

describe('composerNotice', () => {
  it('enables the composer when the gateway is available with the stream capability', () => {
    expect(composerNotice(view('available', true))).toEqual({ enabled: true });
  });
  it('says streaming is off when the gateway is available without the capability', () => {
    expect(composerNotice(view('available', false))).toEqual({ enabled: false, text: STREAMING_OFF_TEXT });
  });
  it.each(['degraded', 'absent'] as const)('says the gateway is not available when it is %s', (state) => {
    expect(composerNotice(view(state, false))).toEqual({ enabled: false, text: GATEWAY_UNAVAILABLE_TEXT });
  });
});
