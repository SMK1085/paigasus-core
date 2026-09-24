// SPDX-License-Identifier: Apache-2.0
//
// The playground composer's rule (SMA-635 spec § 6.1, D7). Pure, like gateway-state.ts: the server
// page computes it and the client component only renders it. The route handler does NOT check the
// capability (D8): the gateway's `400 streaming-disabled` is the authority.
import type { GatewayView } from './gateway-state';

export const STREAMING_OFF_TEXT = 'Streaming is off on this gateway.';
export const GATEWAY_UNAVAILABLE_TEXT = 'The gateway is not available.';

export type ComposerNotice = { readonly enabled: true } | { readonly enabled: false; readonly text: string };

export function composerNotice(view: GatewayView): ComposerNotice {
  if (view.state !== 'available') return { enabled: false, text: GATEWAY_UNAVAILABLE_TEXT };
  if (!view.streaming) return { enabled: false, text: STREAMING_OFF_TEXT };
  return { enabled: true };
}
