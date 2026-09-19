// SPDX-License-Identifier: Apache-2.0
//
// The copy of iam-console's node-status module in this zone (SMA-636 § 9): all nine
// (own × effective) combinations, a status value this build does not know, and the label tables.
import { describe, expect, it } from 'vitest';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import { BADGE_LABEL, COLUMN_LABEL, lifecycleOf, lifecycleView, PARENT_ARCHIVED_NOTE, statusColumnLabel, type LifecycleView, type NodeLifecycle } from '../../app/(console)/node-status';

type Row = { readonly status: NodeStatus; readonly effectiveStatus: NodeStatus; readonly lifecycle: NodeLifecycle; readonly view: LifecycleView };

const ROWS: readonly Row[] = [
  { status: NodeStatus.UNSPECIFIED, effectiveStatus: NodeStatus.UNSPECIFIED, lifecycle: { own: 'unknown', effective: 'unknown' }, view: 'unknown' },
  { status: NodeStatus.UNSPECIFIED, effectiveStatus: NodeStatus.ACTIVE, lifecycle: { own: 'unknown', effective: 'active' }, view: 'unknown' },
  { status: NodeStatus.UNSPECIFIED, effectiveStatus: NodeStatus.ARCHIVED, lifecycle: { own: 'unknown', effective: 'archived' }, view: 'unknown' },
  { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.UNSPECIFIED, lifecycle: { own: 'active', effective: 'unknown' }, view: 'unknown' },
  { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE, lifecycle: { own: 'active', effective: 'active' }, view: 'active' },
  { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ARCHIVED, lifecycle: { own: 'active', effective: 'archived' }, view: 'archived-parent' },
  { status: NodeStatus.ARCHIVED, effectiveStatus: NodeStatus.UNSPECIFIED, lifecycle: { own: 'archived', effective: 'unknown' }, view: 'unknown' },
  // IAM cannot send this pair. If it does, the node still shows as archived (spec § 5.2 rule 2).
  { status: NodeStatus.ARCHIVED, effectiveStatus: NodeStatus.ACTIVE, lifecycle: { own: 'archived', effective: 'active' }, view: 'archived' },
  { status: NodeStatus.ARCHIVED, effectiveStatus: NodeStatus.ARCHIVED, lifecycle: { own: 'archived', effective: 'archived' }, view: 'archived' },
];

describe('lifecycleOf and lifecycleView', () => {
  it('has all nine combinations', () => {
    expect(new Set(ROWS.map((row) => `${String(row.status)}/${String(row.effectiveStatus)}`)).size).toBe(9);
  });

  it.each(ROWS)('status $status / effective $effectiveStatus is $view', ({ status, effectiveStatus, lifecycle, view }) => {
    expect(lifecycleOf({ status, effectiveStatus })).toEqual(lifecycle);
    expect(lifecycleView(lifecycle)).toBe(view);
  });

  it('maps a status value this build does not know to unknown (version skew)', () => {
    const future = 7 as NodeStatus;
    expect(lifecycleOf({ status: future, effectiveStatus: NodeStatus.ACTIVE })).toEqual({ own: 'unknown', effective: 'active' });
    expect(lifecycleView(lifecycleOf({ status: NodeStatus.ACTIVE, effectiveStatus: future }))).toBe('unknown');
  });
});

describe('the badge and column tables (spec § 6.3)', () => {
  it('labels the header badge', () => {
    expect(BADGE_LABEL).toEqual({ active: null, archived: 'Archived', 'archived-parent': 'Archived (parent)', unknown: 'Status unknown' });
  });

  it('labels the Status column', () => {
    expect(COLUMN_LABEL).toEqual({ active: 'Active', archived: 'Archived', 'archived-parent': 'Archived (parent)', unknown: 'Unknown' });
    expect(statusColumnLabel({ own: 'active', effective: 'archived' })).toBe('Archived (parent)');
  });

  it('holds the exact parent-archived note', () => {
    expect(PARENT_ARCHIVED_NOTE).toBe('A parent of this item is archived. Restore the parent to change this item.');
  });
});
