// SPDX-License-Identifier: Apache-2.0
//
// The three error views and the 403 view (spec § 6.1, § 6.2). Server components are async
// functions here, so a test awaits them and renders the element they return.
import type { ReactElement, ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { PaigasusError, Presentation } from '@paigasus/sdk/errors/types';
import Forbidden from '../../app/(console)/forbidden';
import { FORBIDDEN_VIEW_CORRELATION } from '../../lib/correlation';
import { FormError } from '../../app/_components/form-error';
import { PageError } from '../../app/_components/page-error';
import { SectionError } from '../../app/_components/section-error';
import { setRequestHeaders } from '../support/next-headers';

// next/link needs the App Router's context, which a static render does not have.
vi.mock('next/link', () => ({
  default: ({ href, children, ...rest }: { href: string; children?: ReactNode }) => (
    <a href={href} {...rest}>
      {children}
    </a>
  ),
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/orgs/abc' }));

const CID = '0198f2c1-8888-7000-8000-00000000abcd';

function errorWith(presentation: Presentation): PaigasusError {
  return {
    presentation,
    domain: null,
    reason: null,
    rawReason: null,
    rawDomain: null,
    message: 'IAM text that must never show',
    correlationId: CID,
    requestId: null,
    retryable: false,
    metadata: {},
    transport: { kind: 'grpc', code: 7, codeName: 'PermissionDenied' },
  };
}

function render(element: ReactElement): string {
  return renderToStaticMarkup(
    <ZoneProvider zone="iam" zones={{ iam: '/iam' }}>
      {element}
    </ZoneProvider>,
  );
}

describe('PageError', () => {
  it('calls forbidden() for a forbidden read, so Next answers 403', async () => {
    await expect(PageError({ error: errorWith('forbidden') })).rejects.toMatchObject({ digest: 'NEXT_HTTP_ERROR_FALLBACK;403' });
  });

  it('calls notFound() for a not-found read', async () => {
    await expect(PageError({ error: errorWith('not-found') })).rejects.toMatchObject({ digest: 'NEXT_HTTP_ERROR_FALLBACK;404' });
  });

  it.each(['degraded', 'generic', 'conflict', 'invalid-input', 'rate-limited'] as const)('renders ErrorState with the correlation id for %s', async (presentation) => {
    const html = render(await PageError({ error: errorWith(presentation) }));
    expect(html).toContain(CID);
    expect(html).not.toContain('IAM text');
  });

  it('renders the "not enabled" EmptyState for disabled', async () => {
    const html = render(await PageError({ error: errorWith('disabled') }));
    expect(html).toContain('This feature is not enabled on this IAM');
  });

  it('renders a "Sign in again" LINK with a returnTo for relogin, never a redirect', async () => {
    setRequestHeaders({ 'x-paigasus-request-path': '/iam/orgs/abc' });
    const html = render(await PageError({ error: errorWith('relogin') }));
    expect(html).toContain('href="/iam/auth/login?returnTo=%2Fiam%2Forgs%2Fabc"');
  });
});

describe('SectionError', () => {
  it('renders a forbidden section inline, with the correlation id, and does not throw', async () => {
    const html = render(await SectionError({ error: errorWith('forbidden') }));
    expect(html).toContain('data-presentation="forbidden"');
    expect(html).toContain(CID);
  });

  it('renders a not-found section inline', async () => {
    const html = render(await SectionError({ error: errorWith('not-found') }));
    expect(html).toContain('data-presentation="not-found"');
  });
});

describe('FormError', () => {
  it('renders nothing without an error', () => {
    expect(render(<FormError error={null} />)).toBe('');
  });

  it('renders an alert with the copy and the correlation id, never IAM’s message', () => {
    const html = render(<FormError error={errorWith('forbidden')} />);
    expect(html).toContain('role="alert"');
    expect(html).toContain(CID);
    expect(html).not.toContain('IAM text');
  });

  it('keeps the basePath in the returnTo of its "Sign in again" link', () => {
    const html = render(<FormError error={errorWith('relogin')} />);
    expect(html).toContain('href="/iam/auth/login?returnTo=%2Fiam%2Forgs%2Fabc"');
  });
});

describe('the 403 view', () => {
  it('always shows the fixed copy and a way back', async () => {
    const html = render(await Forbidden());
    expect(html).toContain('data-testid="forbidden-view"');
    expect(html).toContain('href="/orgs"');
    expect(html).not.toContain('data-testid="correlation-id"');
  });

  // ONE case for both values of the constant, so no case is ever skipped. In 'header' mode it fails
  // when the view stops reading the header; in 'fallback' mode it fails when the view shows the id.
  it('shows the correlation id proxy.ts set in header mode, and no id in fallback mode (spec § 6.2)', async () => {
    setRequestHeaders({ 'paigasus-correlation-id': CID });
    const html = render(await Forbidden());
    if (FORBIDDEN_VIEW_CORRELATION === 'header') {
      expect(html).toContain(`data-testid="correlation-id">${CID}<`);
    } else {
      expect(html).not.toContain(CID);
    }
  });
});
