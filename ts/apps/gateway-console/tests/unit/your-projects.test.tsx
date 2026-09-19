// SPDX-License-Identifier: Apache-2.0
//
// "Your projects" on the overview (SMA-636 D16): the TEAM and PROJECT entries of myScopes(), a
// project as a link to its settings page, with no prefetch. An organization entry is the switcher's.
import type { ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { IamResult, MyScopes } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { YourProjects, yourProjectRows } from '../../app/_components/your-projects';

vi.mock('next/link', () => ({
  default: ({ href, prefetch, children }: { href: string; prefetch?: boolean; children?: ReactNode }) => (
    <a href={href} data-prefetch={String(prefetch)}>
      {children}
    </a>
  ),
}));

const ORG = '0190a100-0000-7000-8000-00000000000a';
const TEAM = '0190a1b2-0000-7000-8000-0000000000a1';
const PROJECT = '0190a1c3-0000-7000-8000-0000000000a1';

const DENIED_TEAM = '0190a1b2-0000-7000-8000-0000000000b2';
const DENIED_PROJECT = '0190a1c3-0000-7000-8000-0000000000b3';

const SCOPES: IamResult<MyScopes> = {
  ok: true,
  value: {
    entries: [
      { kind: 'organization', prn: `prn:pgs:iam:::organization/${ORG}`, orgId: ORG, label: 'Acme', denied: false },
      { kind: 'team', prn: `prn:pgs:iam::${ORG}:team/${TEAM}`, orgId: ORG, teamId: TEAM, label: 'Platform Team', denied: false },
      { kind: 'project', prn: `prn:pgs:iam::${ORG}:project/${PROJECT}`, orgId: ORG, teamId: TEAM, projectId: PROJECT, label: null, denied: false },
    ],
    hiddenCount: 2,
    grantsListed: true,
  },
};

const SCOPES_WITH_DENIED: IamResult<MyScopes> = {
  ok: true,
  value: {
    entries: [
      { kind: 'team', prn: `prn:pgs:iam::${ORG}:team/${DENIED_TEAM}`, orgId: ORG, teamId: DENIED_TEAM, label: null, denied: true },
      { kind: 'project', prn: `prn:pgs:iam::${ORG}:project/${DENIED_PROJECT}`, orgId: ORG, teamId: null, projectId: DENIED_PROJECT, label: null, denied: true },
    ],
    hiddenCount: 0,
    grantsListed: true,
  },
};

function render(scopes: IamResult<MyScopes>): string {
  return renderToStaticMarkup(
    <ZoneProvider zone="gateway" zones={{ gateway: '/gateway' }}>
      <YourProjects scopes={scopes} basePath="/gateway" />
    </ZoneProvider>,
  );
}

describe('Your projects', () => {
  it('keeps the team and project entries only, and links a project to its settings page', () => {
    expect(yourProjectRows(SCOPES, '/gateway')).toEqual([
      { key: `prn:pgs:iam::${ORG}:team/${TEAM}`, kind: 'team', label: 'Platform Team', href: null },
      { key: `prn:pgs:iam::${ORG}:project/${PROJECT}`, kind: 'project', label: PROJECT, href: `/gateway/orgs/${ORG}/projects/${PROJECT}` },
    ]);
  });

  it('renders a team as a label and a project as a link that does not prefetch, and says how many are hidden', () => {
    const html = render(SCOPES);
    expect(html).toContain('data-testid="your-projects"');
    expect(html).toContain('Team: Platform Team');
    expect(html).toContain(`href="/orgs/${ORG}/projects/${PROJECT}" data-prefetch="false"`);
    expect(html).not.toContain('Acme');
    expect(html).toContain('2 more scopes are not shown.');
  });

  it('says so when there is no team or project scope, and when the scopes could not be loaded', () => {
    expect(render({ ok: true, value: { entries: [], hiddenCount: 0, grantsListed: true } })).toContain('No team or project scopes');
    const error = { presentation: 'degraded' } as PaigasusError;
    expect(render({ ok: false, error })).toContain('Your teams and projects could not be loaded.');
  });

  it('does not link a denied team or project: the link would only lead to a 403', () => {
    expect(yourProjectRows(SCOPES_WITH_DENIED, '/gateway')).toEqual([
      { key: `prn:pgs:iam::${ORG}:team/${DENIED_TEAM}`, kind: 'team', label: 'No access to details', href: null },
      { key: `prn:pgs:iam::${ORG}:project/${DENIED_PROJECT}`, kind: 'project', label: 'No access to details', href: null },
    ]);

    const html = render(SCOPES_WITH_DENIED);
    expect(html).toContain('Team: No access to details');
    expect(html).toContain('Project: No access to details');
    expect(html).not.toContain('<a ');
    expect(html).not.toContain(DENIED_PROJECT);
  });
});
