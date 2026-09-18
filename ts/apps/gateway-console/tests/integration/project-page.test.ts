// SPDX-License-Identifier: Apache-2.0
//
// /gateway/orgs/[org]/projects/[project] — the project settings page (SMA-636 spec § 4.1, § 4.2,
// D5). A failed GetTeam or GetOrganization keeps the page and shows the UUID instead of the name.
import { createElement, type ReactElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import { organizationPrn, projectPrn, resetDiscoveryForTest, teamPrn } from '@paigasus/console-core';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import { denial, type FakeIamHandlers } from '@paigasus/console-core/testing';
import { PageError } from '../../app/_components/page-error';
import { IDS, installSession, startIntegrationEnv, stopIntegrationEnv, type IntegrationEnv } from './support';

const { notFoundMock } = vi.hoisted(() => ({
  notFoundMock: vi.fn(() => {
    throw new Error('called notFound()');
  }),
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), notFound: notFoundMock, usePathname: () => '/orgs' }));
vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => createElement('a', { href }, children),
}));

import ProjectSettingsPage from '../../app/(console)/orgs/[org]/projects/[project]/page';

let env: IntegrationEnv;

beforeAll(async () => {
  env = await startIntegrationEnv();
});

afterAll(async () => {
  await stopIntegrationEnv(env);
});

beforeEach(async () => {
  await installSession();
  notFoundMock.mockClear();
});

afterEach(() => {
  resetDiscoveryForTest();
});

const ORG_A = organizationPrn(IDS.orgA);
const TEAM_A1 = teamPrn(IDS.orgA, IDS.teamA1);
const PROJECT_A1 = projectPrn(IDS.orgA, IDS.projectA1);
const ACTIVE = { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE };

function world(overrides: FakeIamHandlers = {}): FakeIamHandlers {
  return {
    'tenancy.getProject': (req) => ({ project: { prn: req.prn, teamPrn: TEAM_A1, orgPrn: ORG_A, slug: 'gw', name: 'Inference Gateway', ...ACTIVE } }),
    'tenancy.getTeam': () => ({ team: { prn: TEAM_A1, orgPrn: ORG_A, slug: 'platform', name: 'Platform Team', ...ACTIVE } }),
    'tenancy.getOrganization': () => ({ organization: { prn: ORG_A, slug: 'acme', name: 'Acme Corp', ...ACTIVE } }),
    'serviceAccounts.listServiceAccounts': () => ({ serviceAccounts: [] }),
    ...overrides,
  };
}

function render(element: ReactElement): string {
  return renderToStaticMarkup(createElement(ZoneProvider, { zone: 'gateway', zones: { gateway: '/gateway' }, children: element }));
}

function page(project: string = IDS.projectA1): Promise<ReactElement> {
  return ProjectSettingsPage({ params: Promise.resolve({ org: IDS.orgA, project }), searchParams: Promise.resolve({}) });
}

describe('the project settings page', () => {
  it('renders the project, its team, the breadcrumb link to its organization, and a section owned by the project', async () => {
    env.iam.setHandlers(world());
    const before = env.iam.callsTo('serviceAccounts.listServiceAccounts').length;

    const html = render(await page());

    expect(html).toContain('data-testid="project-settings"');
    expect(html).toContain('Inference Gateway');
    expect(html).toContain('Team: Platform Team');
    expect(html).toContain(`<a href="/orgs/${IDS.orgA}">Acme Corp</a>`);
    expect(html).toContain('data-testid="service-accounts"');
    expect(env.iam.callsTo('serviceAccounts.listServiceAccounts').slice(before)[0]?.request).toMatchObject({ ownerPrn: PROJECT_A1 });
  });

  it('shows the UUIDs when GetTeam and GetOrganization are denied, and still renders', async () => {
    env.iam.setHandlers(
      world({
        'tenancy.getTeam': () => {
          throw denial();
        },
        'tenancy.getOrganization': () => {
          throw denial();
        },
      }),
    );

    const html = render(await page());

    expect(html).toContain(`Team: ${IDS.teamA1}`);
    expect(html).toContain(`<a href="/orgs/${IDS.orgA}">${IDS.orgA}</a>`);
  });

  it('returns the page error for a denied project', async () => {
    env.iam.setHandlers(
      world({
        'tenancy.getProject': () => {
          throw denial();
        },
      }),
    );
    expect((await page()).type).toBe(PageError);
  });

  it('calls notFound() for a non-UUID [project] segment, and makes NO IAM call', async () => {
    env.iam.setHandlers(world());
    const before = env.iam.calls.length;

    await expect(page('gateway')).rejects.toThrow('called notFound()');

    expect(env.iam.calls.length).toBe(before);
  });
});
