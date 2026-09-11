// SPDX-License-Identifier: Apache-2.0
//
// The ONE copy of <Capability>'s branch table (SMA-510 spec § 7.1, § 9.2). <Capability> calls it,
// and @paigasus/app-shell's navStateOf calls it, so a nav entry and a <Capability> cannot disagree.
//
// It is reachable from the client-safe ./client entry, so: no directive, no `server-only`, and
// EXTENSIONLESS relative specifiers. A consuming Next app compiles this file, and Turbopack
// (Next 16.3.4) does not map './x.js' to './x.ts' (measured, SMA-510).
import type { CapabilityKey, ServiceState } from '../types';
import { serviceOf } from './service-of';

/** What to do with a feature: render nothing, render it, or render it disabled with a reason. */
export type CapabilityOutcome = 'hidden' | 'shown' | 'degraded';

/**
 *   absent                          -> hidden
 *   available, no need              -> shown
 *   available, need listed          -> shown
 *   available, need not listed      -> hidden   (the service answered: the build lacks it; no outage)
 *   degraded, with or without a descriptor -> degraded (disabled with a reason, never hidden)
 *
 * Throws when `need` belongs to another service than `state`: an `iam.*` key checked against the
 * gateway's state is a programming error, and a silent 'hidden' would hide it.
 */
export function capabilityOutcome(state: ServiceState, need?: CapabilityKey): CapabilityOutcome {
  if (need !== undefined) {
    const owner = serviceOf(need);
    if (owner !== state.service) {
      throw new Error(`capabilityOutcome: capability "${need}" belongs to service "${owner}", but it was checked against the state of "${state.service}"`);
    }
  }
  if (state.state === 'absent') return 'hidden';
  if (state.state === 'available') return need === undefined || state.capabilities.includes(need) ? 'shown' : 'hidden';
  return 'degraded';
}
