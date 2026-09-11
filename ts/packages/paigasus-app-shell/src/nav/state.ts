// SPDX-License-Identifier: Apache-2.0
//
// NO 'use client' directive (spec § 5.2): an app's SERVER layout calls navStateOf on the
// ServiceState it resolved, and passes the result to PrimaryNav as a prop. The fixture's
// /iam/shell page calls it in a server component, which is the browser-tier proof.
import { capabilityOutcome } from '@paigasus/discovery/client';
import type { CapabilityKey, DegradedReason, ServiceState } from '@paigasus/discovery/types';

/** What PrimaryNav needs to know about one entry. It holds no descriptor data (spec § 7.1). */
export type NavEntryState = { readonly state: 'absent' } | { readonly state: 'available' } | { readonly state: 'degraded'; readonly service: string; readonly reason: DegradedReason };

/**
 * <Capability>'s branch table for a data-driven nav entry. An app cannot wrap one entry of a
 * rendered-from-data nav in <Capability>, so this is the capability-level path for nav entries.
 * The table itself is @paigasus/discovery's capabilityOutcome: there is one copy.
 *
 * Throws when `need` belongs to another service than `serviceState` (a programming error).
 */
export function navStateOf(serviceState: ServiceState, need?: CapabilityKey): NavEntryState {
  const outcome = capabilityOutcome(serviceState, need);
  if (outcome === 'hidden') return { state: 'absent' };
  if (outcome === 'shown') return { state: 'available' };
  if (serviceState.state !== 'degraded') {
    // capabilityOutcome answers 'degraded' only for a degraded state. This guard narrows the type.
    throw new Error('navStateOf: capabilityOutcome answered degraded for a state that is not degraded');
  }
  return { state: 'degraded', service: serviceState.service, reason: serviceState.reason };
}
