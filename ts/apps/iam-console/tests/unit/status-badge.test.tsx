// SPDX-License-Identifier: Apache-2.0
//
// The header badge (SMA-630 spec § 6.3): no badge for an active node, one label per other view.
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type { NodeLifecycle } from '../../app/(console)/node-status';
import { StatusBadge } from '../../app/(console)/status-badge';

describe('StatusBadge', () => {
  it('renders nothing for an active node', () => {
    expect(renderToStaticMarkup(<StatusBadge lifecycle={{ own: 'active', effective: 'active' }} />)).toBe('');
  });

  it.each<[string, NodeLifecycle, string]>([
    ['archived', { own: 'archived', effective: 'archived' }, 'Archived'],
    ['archived-parent', { own: 'active', effective: 'archived' }, 'Archived (parent)'],
    ['unknown', { own: 'unknown', effective: 'unknown' }, 'Status unknown'],
  ])('labels %s', (_view, lifecycle, label) => {
    const html = renderToStaticMarkup(<StatusBadge lifecycle={lifecycle} />);
    expect(html).toContain('data-testid="node-status"');
    expect(html).toContain(`>${label}</span>`);
  });
});
