// SPDX-License-Identifier: Apache-2.0
//
// The settings pager (SMA-636 D15, § 4.1): full-path links through ZoneLink, the other list's
// parameters kept, and NO prefetch. The next/link double writes `prefetch` as `data-prefetch`.
import type { ReactElement, ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import { Pager } from '../../app/_components/pager';

vi.mock('next/link', () => ({
  default: ({ href, prefetch, children }: { href: string; prefetch?: boolean; children?: ReactNode }) => (
    <a href={href} data-prefetch={String(prefetch)}>
      {children}
    </a>
  ),
}));

function render(element: ReactElement): string {
  return renderToStaticMarkup(
    <ZoneProvider zone="gateway" zones={{ gateway: '/gateway' }}>
      {element}
    </ZoneProvider>,
  );
}

describe('Pager', () => {
  it('renders nothing on a single page', () => {
    expect(render(<Pager label="Pages" path="/gateway/orgs/o" param="saOffset" offset={0} nextOffset={null} />)).toBe('');
  });

  it('links both ways, keeps the other parameters, and never prefetches', () => {
    const html = render(<Pager label="Pages" path="/gateway/orgs/o" param="saOffset" offset={50} nextOffset={100} keep={{ sa: 'id-1', keyOffset: 0 }} />);
    // ZoneLink hands next/link the zone-relative remainder (Next adds the base path back).
    expect(html).toContain('href="/orgs/o?sa=id-1"');
    expect(html).toContain('href="/orgs/o?sa=id-1&amp;saOffset=100"');
    expect(html.match(/data-prefetch="false"/g)).toHaveLength(2);
  });
});
