// SPDX-License-Identifier: Apache-2.0
//
// COPY of ts/apps/iam-console/app/(console)/node-status.ts (SMA-636 D11, spec § 9). Nothing gates a
// divergence between the two copies. The gateway settings read an owner node's lifecycle here: the
// service-accounts section is read-only unless the owner is active (spec § 5.8).
//
// IAM sends two NodeStatus values per node: `status` (the node itself) and `effectiveStatus`
// (archived when the node OR an ancestor is archived). UNSPECIFIED, and any value that this build
// does not know, map to 'unknown'.
//
// A plain module with no `server-only` import. Its only import is @paigasus/sdk's guard-free
// ./iam/types entry, so a client component can read the note from here.
import { NodeStatus } from '@paigasus/sdk/iam/types';

export type NodeState = 'active' | 'archived' | 'unknown';
export type NodeLifecycle = { readonly own: NodeState; readonly effective: NodeState };
export type LifecycleView = 'active' | 'archived' | 'archived-parent' | 'unknown';

function stateOf(status: NodeStatus): NodeState {
  if (status === NodeStatus.ACTIVE) return 'active';
  if (status === NodeStatus.ARCHIVED) return 'archived';
  return 'unknown';
}

export function lifecycleOf(node: { readonly status: NodeStatus; readonly effectiveStatus: NodeStatus }): NodeLifecycle {
  return { own: stateOf(node.status), effective: stateOf(node.effectiveStatus) };
}

/**
 * The rules of spec § 5.2, in this order:
 * 1. an unknown half gives 'unknown';
 * 2. an archived node gives 'archived' (an archived/active pair cannot come from IAM; if it does,
 *    it still shows as archived);
 * 3. an archived ancestor gives 'archived-parent';
 * 4. else 'active'.
 */
export function lifecycleView(lifecycle: NodeLifecycle): LifecycleView {
  if (lifecycle.own === 'unknown' || lifecycle.effective === 'unknown') return 'unknown';
  if (lifecycle.own === 'archived') return 'archived';
  if (lifecycle.effective === 'archived') return 'archived-parent';
  return 'active';
}

/** The badge next to the `h1` of a detail page. An active node gets no badge. */
export const BADGE_LABEL: Readonly<Record<LifecycleView, string | null>> = {
  active: null,
  archived: 'Archived',
  'archived-parent': 'Archived (parent)',
  unknown: 'Status unknown',
};

/** The Status column of the organization, team and project tables. */
export const COLUMN_LABEL: Readonly<Record<LifecycleView, string>> = {
  active: 'Active',
  archived: 'Archived',
  'archived-parent': 'Archived (parent)',
  unknown: 'Unknown',
};

export function statusColumnLabel(lifecycle: NodeLifecycle): string {
  return COLUMN_LABEL[lifecycleView(lifecycle)];
}

/**
 * The Manage section shows this for 'archived-parent', also when it has no control (spec § 5.3).
 * It names no ancestor: the page does not know which ancestor is archived.
 */
export const PARENT_ARCHIVED_NOTE = 'A parent of this item is archived. Restore the parent to change this item.';
