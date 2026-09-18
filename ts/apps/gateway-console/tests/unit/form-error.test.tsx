// SPDX-License-Identifier: Apache-2.0
//
// FormError and SectionError, the copies of iam-console's (SMA-636 spec § 9). FormError has ONE
// addition: a `message` that replaces the copy, for the results whose words depend on the control.
import type { ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import { ErrorReason, type PaigasusError, type Presentation } from '@paigasus/sdk/errors/types';
import { FormError } from '../../app/_components/form-error';
import { SectionError } from '../../app/_components/section-error';

vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => <a href={href}>{children}</a>,
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/orgs/o' }));

const CID = '0198f2c1-8888-7000-8000-00000000abcd';

function errorWith(presentation: Presentation, reason: ErrorReason | null = null): PaigasusError {
  return {
    presentation,
    domain: null,
    reason,
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

function render(node: ReactNode): string {
  return renderToStaticMarkup(
    <ZoneProvider zone="gateway" zones={{ gateway: '/gateway' }}>
      {node}
    </ZoneProvider>,
  );
}

describe('FormError', () => {
  it('renders nothing without an error', () => {
    expect(render(<FormError error={null} />)).toBe('');
  });

  it('shows the reason copy and the correlation id, never IAM’s message', () => {
    const html = render(<FormError error={errorWith('conflict', ErrorReason.SERVICE_ACCOUNT_NAME_CONFLICT)} />);
    expect(html).toContain('A service account with this name already exists here.');
    expect(html).toContain(`data-testid="correlation-id">${CID}<`);
    expect(html).not.toContain('IAM text');
  });

  it('shows a given message in place of the copy (SMA-636 § 5.3, § 5.4)', () => {
    const html = render(<FormError error={errorWith('generic')} message="Model calls may already be allowed. The page was reloaded." />);
    expect(html).toContain('Model calls may already be allowed. The page was reloaded.');
    expect(html).not.toContain('The request failed.');
  });

  it('links "Sign in again" back to this page, inside the zone, for relogin', () => {
    expect(render(<FormError error={errorWith('relogin')} />)).toContain('href="/gateway/auth/login?returnTo=%2Fgateway%2Forgs%2Fo"');
  });
});

describe('SectionError', () => {
  it('shows the error state with the correlation id for a refused section, and the empty state for a disabled one', async () => {
    const refused = renderToStaticMarkup(await SectionError({ error: errorWith('degraded') }));
    expect(refused).toContain('data-testid="section-error"');
    expect(refused).toContain('data-presentation="degraded"');
    expect(refused).toContain(CID);
    expect(refused).not.toContain('IAM text');

    const disabled = renderToStaticMarkup(await SectionError({ error: errorWith('disabled') }));
    expect(disabled).toContain('This feature is not enabled on this IAM');
  });
});
