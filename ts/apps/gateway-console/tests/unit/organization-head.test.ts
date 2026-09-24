// SPDX-License-Identifier: Apache-2.0
//
// The org page's head, shared with the playground page (SMA-635 spec § 6.1).
import { afterEach, describe, expect, it, vi } from 'vitest';
import { Code } from '@connectrpc/connect';
import { logger } from '@paigasus/console-core';
import { denial } from '@paigasus/console-core/testing';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import { loadOrganizationHead } from '../../app/(console)/orgs/[org]/load';

type Tenancy = Parameters<typeof loadOrganizationHead>[0];
const ORG = '0190A100-0000-7000-8000-0000000000E1';

afterEach(() => {
  vi.restoreAllMocks();
});

function tenancy(answer: () => Promise<unknown>): { tenancy: Tenancy; getOrganization: ReturnType<typeof vi.fn> } {
  const getOrganization = vi.fn(answer);
  return { tenancy: { getOrganization } as unknown as Tenancy, getOrganization };
}

describe('loadOrganizationHead', () => {
  it('is not-found for a value that is not a UUID, with no IAM call', async () => {
    const t = tenancy(() => Promise.resolve({}));
    expect(await loadOrganizationHead(t.tenancy, 'acme')).toEqual({ kind: 'not-found' });
    expect(t.getOrganization).not.toHaveBeenCalled();
  });

  it('answers the lower-case id and the org PRN', async () => {
    const t = tenancy(() => Promise.resolve({ organization: { prn: 'x', slug: 'acme', name: 'Acme', status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE } }));
    const head = await loadOrganizationHead(t.tenancy, ORG);
    expect(head).toMatchObject({ kind: 'ok', orgId: ORG.toLowerCase(), orgPrn: `prn:pgs:iam:::organization/${ORG.toLowerCase()}`, organization: { name: 'Acme', slug: 'acme' } });
  });

  it('is an error for a denial, and not-found for a not-found answer', async () => {
    vi.spyOn(logger, 'appEvent').mockImplementation(() => undefined);
    const denied = await loadOrganizationHead(tenancy(() => Promise.reject(denial())).tenancy, ORG);
    expect(denied.kind === 'error' ? denied.error.presentation : null).toBe('forbidden');
    expect(await loadOrganizationHead(tenancy(() => Promise.reject(denial({ code: Code.NotFound, reason: 'not-found' }))).tenancy, ORG)).toEqual({ kind: 'not-found' });
  });
});
