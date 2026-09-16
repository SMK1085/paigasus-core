// SPDX-License-Identifier: Apache-2.0
//
// /gateway/orgs/[org] (spec § 7.3). A denied GetOrganization is why forbidden.tsx is reachable in
// this zone: PageError calls forbidden() for a `forbidden` presentation, and Next renders the 403
// boundary inside the (console) layout. next/navigation's forbidden() and notFound() are MOCKED so
// a case asserts the SPECIFIC call was made (with no argument, matching the real signature), not
// merely that something threw — a broken page that throws a different error for the wrong reason
// must not pass these cases.
//
// This file is `.ts`, not `.tsx` (the brief's own file list), so the next/link double below uses
// createElement rather than JSX.
import { createElement, type ReactElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { Code } from '@connectrpc/connect';
import { ZoneProvider } from '@paigasus/app-shell';
import { organizationPrn, resetDiscoveryForTest } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors';
import { denial } from '@paigasus/console-core/testing';
import { PageError } from '../../app/_components/page-error';
import { IDS, installSession, startIntegrationEnv, stopIntegrationEnv, type IntegrationEnv } from './support';

const { forbiddenMock, notFoundMock } = vi.hoisted(() => ({
  forbiddenMock: vi.fn(() => {
    throw new Error('called forbidden()');
  }),
  notFoundMock: vi.fn(() => {
    throw new Error('called notFound()');
  }),
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), forbidden: forbiddenMock, notFound: notFoundMock }));
// ZoneLink (inside Breadcrumbs) renders next/link, which needs the App Router's context that a
// static render does not have — same double as tests/unit/error-views.test.tsx.
vi.mock('next/link', () => ({
  default: ({ href, children, ...rest }: { href: string; children?: ReactNode }) => createElement('a', { href, ...rest }, children),
}));

import ScopePage from '../../app/(console)/orgs/[org]/page';

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

describe('ScopePage', () => {
  it('renders the breadcrumbs with the org name, and the overview beneath it, scoped to that org', async () => {
    env.iam.setHandlers({ 'tenancy.getOrganization': (req: { prn: string }) => ({ organization: { prn: req.prn, slug: 'acme', name: 'Acme Corp' } }) });

    const html = render(await ScopePage({ params: Promise.resolve({ org: IDS.orgA }) }));

    expect(html).toContain('Acme Corp');
    expect(html).toContain('data-testid="zone-overview"');
    expect(html).toContain('data-testid="gateway-scope">Acme Corp<');
    const calls = env.iam.callsTo('tenancy.getOrganization');
    expect(calls).toHaveLength(1);
    expect(calls[0]?.request).toMatchObject({ prn: ORG_A });
  });

  it('calls forbidden() when GetOrganization is denied, and the correlation id survives into the error the view would render', async () => {
    const sentId = '0198f2c1-8888-7000-8000-000000000099';
    env.iam.setHandlers({
      'tenancy.getOrganization': () => {
        throw denial({ code: Code.PermissionDenied, reason: 'forbidden', correlationId: sentId });
      },
    });

    const element = await ScopePage({ params: Promise.resolve({ org: IDS.orgA }) });

    expect(element.type).toBe(PageError);
    const { error } = element.props as { error: PaigasusError };
    expect(error.presentation).toBe('forbidden');
    expect(error.correlationId).toBe(sentId);

    // Rendering what the page returned is what actually invokes PageError, which is the ONE place
    // that calls forbidden() — see page-error.tsx's own header.
    await expect(PageError(element.props as { error: PaigasusError })).rejects.toThrow('called forbidden()');
    expect(forbiddenMock).toHaveBeenCalledTimes(1);
    expect(forbiddenMock).toHaveBeenCalledWith();
    expect(notFoundMock).not.toHaveBeenCalled();
  });

  it('calls notFound() for a non-UUID [org] segment, and makes NO IAM call', async () => {
    env.iam.setHandlers({ 'tenancy.getOrganization': (req: { prn: string }) => ({ organization: { prn: req.prn, slug: 'acme', name: 'Acme Corp' } }) });
    const before = env.iam.callsTo('tenancy.getOrganization').length;

    await expect(ScopePage({ params: Promise.resolve({ org: 'not-a-uuid' }) })).rejects.toThrow('called notFound()');

    expect(notFoundMock).toHaveBeenCalledTimes(1);
    expect(notFoundMock).toHaveBeenCalledWith();
    expect(env.iam.callsTo('tenancy.getOrganization')).toHaveLength(before);
  });

  it('calls notFound(), not the error view, when IAM answers invalid-input (prn-mismatch)', async () => {
    env.iam.setHandlers({
      'tenancy.getOrganization': () => {
        throw denial({ code: Code.InvalidArgument, reason: 'prn-mismatch' });
      },
    });

    await expect(ScopePage({ params: Promise.resolve({ org: IDS.orgA }) })).rejects.toThrow('called notFound()');

    expect(notFoundMock).toHaveBeenCalledTimes(1);
    expect(forbiddenMock).not.toHaveBeenCalled();
  });
});
