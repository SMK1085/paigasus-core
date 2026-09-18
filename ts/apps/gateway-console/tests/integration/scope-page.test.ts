// SPDX-License-Identifier: Apache-2.0
//
// /gateway/orgs/[org] — the organization settings page (SMA-636 spec § 4.1, § 4.2). SMA-636
// § 7.4 re-baselines this file: SMA-512's scope route rendered the zone overview here. A denied
// GetOrganization is still why forbidden.tsx is reachable in this zone: PageError calls forbidden()
// for a `forbidden` presentation. next/navigation's forbidden() and notFound() are MOCKED, so a
// case asserts the SPECIFIC call was made (with no argument), not merely that something threw.
//
// This file is `.ts`, not `.tsx`, so the next/link double below uses createElement rather than JSX.
import { createElement, type ReactElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { Code } from '@connectrpc/connect';
import { ZoneProvider } from '@paigasus/app-shell';
import { organizationPrn, projectPrn, resetDiscoveryForTest, teamPrn } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import { denial, type FakeIamHandlers } from '@paigasus/console-core/testing';
import { PageError } from '../../app/_components/page-error';
import { serviceAccountPrn } from '../../app/(console)/service-accounts/service-account-id';
import { IDS, installSession, startIntegrationEnv, stopIntegrationEnv, type IntegrationEnv } from './support';

const { forbiddenMock, notFoundMock } = vi.hoisted(() => ({
  forbiddenMock: vi.fn(() => {
    throw new Error('called forbidden()');
  }),
  notFoundMock: vi.fn(() => {
    throw new Error('called notFound()');
  }),
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), forbidden: forbiddenMock, notFound: notFoundMock, usePathname: () => '/orgs' }));
// ZoneLink renders next/link, which needs the App Router's context that a static render does not have.
vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => createElement('a', { href }, children),
}));

import OrganizationSettingsPage from '../../app/(console)/orgs/[org]/page';

let env: IntegrationEnv;

beforeAll(async () => {
  env = await startIntegrationEnv();
});

afterAll(async () => {
  await stopIntegrationEnv(env);
});

beforeEach(async () => {
  await installSession();
  forbiddenMock.mockClear();
  notFoundMock.mockClear();
});

afterEach(() => {
  resetDiscoveryForTest();
});

function render(element: ReactElement): string {
  return renderToStaticMarkup(createElement(ZoneProvider, { zone: 'gateway', zones: { gateway: '/gateway' }, children: element }));
}

const ORG_A = organizationPrn(IDS.orgA);
const TEAM_A1 = teamPrn(IDS.orgA, IDS.teamA1);
const PROJECT_A1 = projectPrn(IDS.orgA, IDS.projectA1);
const SA_A = serviceAccountPrn(IDS.saA);
const ACTIVE = { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE };

function world(overrides: FakeIamHandlers = {}): FakeIamHandlers {
  return {
    'tenancy.getOrganization': (req) => ({ organization: { prn: req.prn, slug: 'acme', name: 'Acme Corp', ...ACTIVE } }),
    'tenancy.listTeams': () => ({ teams: [{ prn: TEAM_A1, orgPrn: ORG_A, slug: 'platform', name: 'Platform Team', ...ACTIVE }] }),
    'tenancy.listProjects': () => ({ projects: [{ prn: PROJECT_A1, teamPrn: TEAM_A1, orgPrn: ORG_A, slug: 'gw', name: 'Inference Gateway', ...ACTIVE }] }),
    'serviceAccounts.listServiceAccounts': () => ({ serviceAccounts: [{ prn: SA_A, ownerPrn: ORG_A, name: 'ci-bot', status: 'active' }] }),
    'serviceAccounts.getServiceAccount': () => ({ serviceAccount: { prn: SA_A, ownerPrn: ORG_A, name: 'ci-bot', status: 'active' } }),
    ...overrides,
  };
}

function page(org: string, query: Record<string, string> = {}): Promise<ReactElement> {
  return OrganizationSettingsPage({ params: Promise.resolve({ org }), searchParams: Promise.resolve(query) });
}

describe('the organization settings page', () => {
  it('renders the header, the gateway line, the service accounts and the projects — not the zone overview', async () => {
    env.iam.setHandlers(world());

    const html = render(await page(IDS.orgA));

    expect(html).toContain('data-testid="org-settings"');
    expect(html).toContain('Acme Corp');
    expect(html).toContain('data-testid="gateway-state-line"');
    expect(html).toContain('data-testid="service-accounts"');
    expect(html).toContain('ci-bot');
    expect(html).toContain('data-testid="projects"');
    expect(html).toContain('Platform Team');
    // ZoneLink hands next/link the zone-relative remainder.
    expect(html).toContain(`href="/orgs/${IDS.orgA}/projects/${IDS.projectA1}"`);
    expect(html).not.toContain('data-testid="zone-overview"');
    // The single-zone map of this tier has no IAM zone, so there is no "Manage in IAM" link (§ 4.4).
    expect(html).not.toContain('data-testid="manage-in-iam"');
    expect(env.iam.callsTo('tenancy.getOrganization').at(-1)?.request).toMatchObject({ prn: ORG_A });
  });

  it('loads the account that ?sa= selects', async () => {
    env.iam.setHandlers(world());
    const before = env.iam.callsTo('serviceAccounts.getServiceAccount').length;

    const html = render(await page(IDS.orgA, { sa: IDS.saA }));

    expect(html).toContain('data-testid="sa-panel"');
    const calls = env.iam.callsTo('serviceAccounts.getServiceAccount').slice(before);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.request).toMatchObject({ prn: SA_A });
  });

  it('shows only the section denial, and still renders the page, when ListServiceAccounts is forbidden', async () => {
    env.iam.setHandlers(
      world({
        'serviceAccounts.listServiceAccounts': () => {
          throw denial();
        },
      }),
    );

    const html = render(await page(IDS.orgA));

    expect(html).toContain('data-testid="service-accounts-denied"');
    expect(html).toContain('You cannot view service accounts here.');
    expect(html).toContain('data-testid="projects"');
  });

  it('calls forbidden() when GetOrganization is denied, and the correlation id survives into the error the view would render', async () => {
    const sentId = '0198f2c1-8888-7000-8000-000000000099';
    env.iam.setHandlers(
      world({
        'tenancy.getOrganization': () => {
          throw denial({ code: Code.PermissionDenied, reason: 'forbidden', correlationId: sentId });
        },
      }),
    );

    const element = await page(IDS.orgA);

    expect(element.type).toBe(PageError);
    const { error } = element.props as { error: PaigasusError };
    expect(error.presentation).toBe('forbidden');
    expect(error.correlationId).toBe(sentId);
    await expect(PageError(element.props as { error: PaigasusError })).rejects.toThrow('called forbidden()');
    expect(forbiddenMock).toHaveBeenCalledTimes(1);
    expect(forbiddenMock).toHaveBeenCalledWith();
    expect(notFoundMock).not.toHaveBeenCalled();
  });

  it('calls notFound() for a non-UUID [org] segment, and makes NO IAM call', async () => {
    env.iam.setHandlers(world());
    const before = env.iam.calls.length;

    await expect(page('not-a-uuid')).rejects.toThrow('called notFound()');

    expect(notFoundMock).toHaveBeenCalledTimes(1);
    expect(notFoundMock).toHaveBeenCalledWith();
    expect(env.iam.calls.length).toBe(before);
  });

  it('calls notFound(), not the error view, when IAM answers invalid-input (prn-mismatch)', async () => {
    env.iam.setHandlers(
      world({
        'tenancy.getOrganization': () => {
          throw denial({ code: Code.InvalidArgument, reason: 'prn-mismatch' });
        },
      }),
    );

    await expect(page(IDS.orgA)).rejects.toThrow('called notFound()');

    expect(notFoundMock).toHaveBeenCalledTimes(1);
    expect(forbiddenMock).not.toHaveBeenCalled();
  });
});
