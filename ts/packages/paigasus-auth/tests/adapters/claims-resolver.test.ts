// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { claimsPrincipalResolver } from '../../src/adapters/claims-resolver.js';

describe('claimsPrincipalResolver', () => {
  it('derives issuer and subject from the claims', async () => {
    const p = await claimsPrincipalResolver.resolve({
      accessToken: 'AT',
      idTokenClaims: { iss: 'https://idp/realms/x', sub: 'user-1' },
    });
    expect(p.issuer).toBe('https://idp/realms/x');
    expect(p.subject).toBe('user-1');
  });

  it('reports no PRN — only IAM can mint one', async () => {
    const p = await claimsPrincipalResolver.resolve({ accessToken: 'AT', idTokenClaims: { iss: 'i', sub: 's' } });
    expect(p.principalPrn).toBeNull();
  });

  it('reports grants as UNAVAILABLE, not as empty-and-known', async () => {
    const p = await claimsPrincipalResolver.resolve({ accessToken: 'AT', idTokenClaims: { iss: 'i', sub: 's' } });
    expect(p.grantsAvailable).toBe(false);
    expect(p.roleGrants).toEqual([]);
    expect(p.memberships).toEqual([]);
  });

  it('never returns the access token in the principal', async () => {
    const p = await claimsPrincipalResolver.resolve({ accessToken: 'AT-secret', idTokenClaims: { iss: 'i', sub: 's' } });
    expect(JSON.stringify(p)).not.toContain('AT-secret');
  });
});
