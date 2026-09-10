// SPDX-License-Identifier: Apache-2.0
import 'server-only';

import type { ReactElement, ReactNode } from 'react';
import { CapabilityDisabled } from './disabled.js';
import { serviceOf } from './core/state.js';
import type { Discovery } from './server.js';
import type { CapabilityKey, DegradedReason } from './types.js';

export { CapabilityDisabled } from './disabled.js';

export type CapabilityProps = {
  readonly discovery: Discovery;
  readonly need: CapabilityKey;
  readonly token: string;
  readonly children: ReactNode;
  /** Replace the default disabled wrapper entirely. SMA-510's nav is the expected first user. */
  readonly degraded?: (reason: DegradedReason) => ReactNode;
};

/**
 * Gate feature UI on a service capability.
 *
 * COSMETIC ONLY, exactly like `can()`. The server remains authoritative and must handle an
 * unimplemented call gracefully; a client that treated this as a security boundary would be wrong.
 *
 * It branches on the FULL state, never on `hasCapability`:
 *
 *   absent                          -> nothing
 *   available, key present          -> children
 *   available, key absent           -> nothing
 *   degraded, descriptor has key    -> children, disabled with a reason
 *   degraded, no descriptor at all  -> children, disabled with a reason
 *
 * That last row is a DECISION, not a derivation: we do not know whether a never-reached service
 * has the capability. Disabling is chosen over hiding because hiding a deployed-but-down service
 * turns an outage into an apparent configuration change, and a wrongly-shown disabled item is
 * recoverable while a wrongly-hidden one is invisible.
 *
 * The `available, key absent` row hides rather than disables, and the asymmetry is deliberate:
 * there the service answered and told us it lacks the feature, so there is no outage to report.
 */
export async function Capability(props: CapabilityProps): Promise<ReactElement | null> {
  const { discovery, need, token, children, degraded } = props;
  // serviceOf, not an inline slice: one definition of "the service a key belongs to", shared with
  // hasCapability and with the config-key validation, so the three cannot drift.
  const state = await discovery.getServiceState(serviceOf(need), token);

  if (state.state === 'absent') return null;
  if (state.state === 'available') {
    return state.capabilities.includes(need) ? <>{children}</> : null;
  }

  if (degraded !== undefined) return <>{degraded(state.reason)}</>;
  return (
    <CapabilityDisabled service={state.service} reason={state.reason}>
      {children}
    </CapabilityDisabled>
  );
}
