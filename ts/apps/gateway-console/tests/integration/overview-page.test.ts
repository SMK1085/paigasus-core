// SPDX-License-Identifier: Apache-2.0
//
// /gateway/overview against the REAL discovery() and myScopes(), with a scripted fake gateway (spec
// § 10.3) and a scripted fake IAM (SMA-636 D16). The page is awaited, then the element it returns is
// rendered with renderToStaticMarkup inside a ZoneProvider, because "Your projects" renders ZoneLink.
//
// RECORDED LIMIT (spec § 10.3): the `absent` state of the gateway view is UNREACHABLE at this tier —
// lib/config.ts refuses a PAIGASUS_SERVICES map with no `gateway` entry. That branch is covered only
// by tests/unit/zone-overview.test.tsx.
//
// This file is `.ts`, not `.tsx`, so the next/link double below uses createElement rather than JSX.
import { createElement, type ReactElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import { organizationPrn, projectPrn, resetDiscoveryForTest, teamPrn } from '@paigasus/console-core';
import { IDS, installSession, startIntegrationEnv, stopIntegrationEnv, type IntegrationEnv } from './support';

vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => createElement('a', { href }, children),
}));

const { default: OverviewPage } = await import('../../app/(console)/overview/page');

let env: IntegrationEnv;

beforeAll(async () => {
  env = await startIntegrationEnv();
});

afterAll(async () => {
  await stopIntegrationEnv(env);
});

beforeEach(async () => {
  await installSession();
  // Every case starts from a healthy, capable gateway and an IAM with no scripted scope, so a case
  // that forgets to script anything fails loudly rather than inheriting the PREVIOUS case's state.
  env.gateway.setReachable(true);
  env.gateway.setServiceInfo({ service: 'gateway', version: '0.0.0-fake', capabilities: ['gateway.chat.stream'] });
  env.iam.setHandlers({});
});

afterEach(() => {
  resetDiscoveryForTest();
});

function render(element: ReactElement): string {
  return renderToStaticMarkup(createElement(ZoneProvider, { zone: 'gateway', zones: { gateway: '/gateway' }, children: element }));
}

describe('OverviewPage', () => {
  it('reports available with streaming and the version when the descriptor lists gateway.chat.stream', async () => {
    env.gateway.setServiceInfo({ service: 'gateway', version: '1.2.3', capabilities: ['gateway.chat.stream'] });

    const html = render(await OverviewPage());

    expect(html).toContain('data-state="available"');
    expect(html).toContain('data-streaming="true"');
    expect(html).toContain('1.2.3');
  });

  it('reports available without streaming when the descriptor omits the capability', async () => {
    env.gateway.setServiceInfo({ service: 'gateway', version: '1.2.3', capabilities: [] });

    const html = render(await OverviewPage());

    expect(html).toContain('data-state="available"');
    expect(html).toContain('data-streaming="false"');
  });

  it('reports degraded with a rendered reason when the gateway answers a bad status', async () => {
    env.gateway.setServiceInfo({ status: 503 });

    const html = render(await OverviewPage());

    expect(html).toContain('data-state="degraded"');
    expect(html).toContain('data-testid="gateway-reason"');
  });

  it('reports degraded with a rendered reason when the gateway is unreachable', async () => {
    env.gateway.setReachable(false);

    const html = render(await OverviewPage());

    expect(html).toContain('data-state="degraded"');
    expect(html).toContain('data-testid="gateway-reason"');
  });

  it('lists the team and project scopes of myScopes() as "Your projects", a project as a link to its page (D16)', async () => {
    const ORG_A = organizationPrn(IDS.orgA);
    const TEAM_A1 = teamPrn(IDS.orgA, IDS.teamA1);
    const PROJECT_A1 = projectPrn(IDS.orgA, IDS.projectA1);
    env.iam.setHandlers({
      // myScopes() sources memberships from currentPrincipal(), which calls WhoAmI (SMA-632).
      'authn.whoAmI': () => ({
        memberships: [
          { id: '0190a1d4-0000-7000-8000-0000000000c1', principalPrn: 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0', nodePrn: TEAM_A1 },
          { id: '0190a1d4-0000-7000-8000-0000000000c2', principalPrn: 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0', nodePrn: PROJECT_A1 },
        ],
      }),
      'tenancy.getTeam': () => ({ team: { prn: TEAM_A1, orgPrn: ORG_A, slug: 'platform', name: 'Platform Team' } }),
      'tenancy.getProject': () => ({ project: { prn: PROJECT_A1, teamPrn: TEAM_A1, orgPrn: ORG_A, slug: 'gw', name: 'Inference Gateway' } }),
    });

    const html = render(await OverviewPage());

    expect(html).toContain('data-testid="your-projects"');
    expect(html).toContain('Team: Platform Team');
    expect(html).toContain(`<a href="/orgs/${IDS.orgA}/projects/${IDS.projectA1}">Inference Gateway</a>`);
  });
});
