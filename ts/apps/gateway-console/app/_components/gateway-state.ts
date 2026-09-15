// SPDX-License-Identifier: Apache-2.0
//
// The pure reader over the gateway's ServiceState (spec § 6). It holds the ONE capability rule this
// zone has: `gateway.chat.stream` counts only when the service is AVAILABLE. A degraded service may
// still carry the descriptor of its last good probe, and treating that as a live capability would
// ship a control that reports a feature the gateway cannot currently serve. This is the same rule
// @paigasus/console-core's cedarCapabilityOf applies to iam.authz.cedar.
//
// No 'server-only': this is a pure function over plain data and the view renders it on the server.
import type { ServiceState } from '@paigasus/discovery/types';

/** The gateway's only capability (rs/crates/services/paigasus-gateway/src/service_info.rs:19-33). */
export const STREAM_CAPABILITY = 'gateway.chat.stream';

export type GatewayView = {
  readonly state: ServiceState['state'];
  /** Why it is degraded, or null in every other state. */
  readonly reason: string | null;
  readonly version: string | null;
  readonly capabilities: readonly string[];
  readonly streaming: boolean;
};

export function gatewayView(state: ServiceState): GatewayView {
  if (state.state === 'absent') return { state: 'absent', reason: null, version: null, capabilities: [], streaming: false };
  return {
    state: state.state,
    reason: state.state === 'degraded' ? state.reason : null,
    version: state.descriptor?.version ?? null,
    capabilities: state.capabilities,
    streaming: state.state === 'available' && state.capabilities.includes(STREAM_CAPABILITY),
  };
}
