// SPDX-License-Identifier: Apache-2.0
//
// The Manage section (SMA-630 spec § 5.3), row by row. Its controls follow two inputs: mayI()
// (`can`) and the node's own status (D7). A server component, so the test renders it statically.
import type { ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { FormAction } from '@paigasus/console-core';
import { ManageSection, type ManageSectionProps } from '../../app/(console)/manage-section';
import { PARENT_ARCHIVED_NOTE, type NodeLifecycle } from '../../app/(console)/node-status';

vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => <a href={href}>{children}</a>,
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/orgs' }));

const noop: FormAction = () => Promise.resolve(null);

const LIFECYCLE = {
  active: { own: 'active', effective: 'active' },
  archived: { own: 'archived', effective: 'archived' },
  'archived-parent': { own: 'active', effective: 'archived' },
  unknown: { own: 'unknown', effective: 'active' },
} as const satisfies Record<string, NodeLifecycle>;

type Can = ManageSectionProps['can'];
const ALL: Can = { rename: true, archive: true, restore: true };
const NONE: Can = { rename: false, archive: false, restore: false };

function render(lifecycle: NodeLifecycle, can: Can): string {
  return renderToStaticMarkup(
    <ZoneProvider zone="iam" zones={{ iam: '/iam' }}>
      <ManageSection node="team" prn="prn:pgs:iam::o:team/t" name="Platform Team" slug="platform" lifecycle={lifecycle} can={can} actions={{ rename: noop, archive: noop, restore: noop }} />
    </ZoneProvider>,
  );
}

type Shown = { readonly rename: boolean; readonly archive: boolean; readonly restore: boolean; readonly note: boolean };

function shown(html: string): Shown {
  return {
    rename: html.includes('data-testid="rename-team"'),
    archive: html.includes('data-testid="archive-team"'),
    restore: html.includes('data-testid="restore-team"'),
    note: html.includes(PARENT_ARCHIVED_NOTE),
  };
}

type Row = { readonly label: string; readonly lifecycle: NodeLifecycle; readonly can: Can; readonly expected: Shown };

// spec § 5.3: `active` → Archive; `archived` → Restore; `archived-parent` → Archive and the note;
// `unknown` → no lifecycle control. The rename form follows `can.rename` in every row.
const ROWS: readonly Row[] = [
  { label: 'active, all allowed', lifecycle: LIFECYCLE.active, can: ALL, expected: { rename: true, archive: true, restore: false, note: false } },
  { label: 'active, rename denied', lifecycle: LIFECYCLE.active, can: { ...ALL, rename: false }, expected: { rename: false, archive: true, restore: false, note: false } },
  { label: 'active, archive denied', lifecycle: LIFECYCLE.active, can: { ...ALL, archive: false }, expected: { rename: true, archive: false, restore: false, note: false } },
  { label: 'archived, all allowed', lifecycle: LIFECYCLE.archived, can: ALL, expected: { rename: true, archive: false, restore: true, note: false } },
  { label: 'archived, restore denied', lifecycle: LIFECYCLE.archived, can: { ...ALL, restore: false }, expected: { rename: true, archive: false, restore: false, note: false } },
  { label: 'archived, only restore allowed', lifecycle: LIFECYCLE.archived, can: { ...NONE, restore: true }, expected: { rename: false, archive: false, restore: true, note: false } },
  { label: 'archived-parent, all allowed', lifecycle: LIFECYCLE['archived-parent'], can: ALL, expected: { rename: true, archive: true, restore: false, note: true } },
  { label: 'archived-parent, archive denied', lifecycle: LIFECYCLE['archived-parent'], can: { ...ALL, archive: false }, expected: { rename: true, archive: false, restore: false, note: true } },
  { label: 'unknown, all allowed', lifecycle: LIFECYCLE.unknown, can: ALL, expected: { rename: true, archive: false, restore: false, note: false } },
  { label: 'unknown, rename denied', lifecycle: LIFECYCLE.unknown, can: { ...ALL, rename: false }, expected: { rename: false, archive: false, restore: false, note: false } },
];

describe('ManageSection (spec § 5.3)', () => {
  it.each(ROWS)('$label', ({ lifecycle, can, expected }) => {
    const html = render(lifecycle, can);
    expect(shown(html)).toEqual(expected);
    // A row with no rename form, no lifecycle control and no note (spec § 5.3: 'unknown' shows a
    // control never, so with can.rename false there is nothing left) renders nothing at all — the
    // same rule the dedicated "renders nothing" case below asserts for the same combination.
    const rendersSomething = expected.rename || expected.archive || expected.restore || expected.note;
    if (rendersSomething) {
      expect(html).toContain('>Manage</h2>');
    } else {
      expect(html).toBe('');
    }
  });

  it('renders nothing when it has no control and no note', () => {
    expect(render(LIFECYCLE.active, NONE)).toBe('');
    expect(render(LIFECYCLE.archived, { ...ALL, restore: false, rename: false })).toBe('');
    expect(render(LIFECYCLE.unknown, { ...NONE, archive: true, restore: true })).toBe('');
  });

  it('renders only the note for archived-parent with no control (the default policy, spec F9)', () => {
    const html = render(LIFECYCLE['archived-parent'], { ...NONE, restore: true });
    expect(shown(html)).toEqual({ rename: false, archive: false, restore: false, note: true });
    expect(html).toContain('data-testid="manage-note"');
  });

  it('names the controls after the node, and passes the name to the archive control', () => {
    const html = render(LIFECYCLE.active, ALL);
    expect(html).toContain('aria-label="Rename team"');
    expect(html).toContain('data-testid="rename-team-error"');
    expect(html).toContain('data-testid="archive-team-error"');
    // The result region (spec § 6.4) renders with the controls, empty before any action.
    expect(html).toContain('<div data-testid="manage-result"></div>');
    expect(html).toContain('>Archive</button>');
    expect(html).not.toContain('Confirm archive');
  });
});
