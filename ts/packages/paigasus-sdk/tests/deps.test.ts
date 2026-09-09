// SPDX-License-Identifier: Apache-2.0
//
// TEMPORARY. Re-confirms spec § 13's M2 and M3 against the WORKSPACE resolution rather than the
// standalone `npm pack` tree they were first measured in — spec § 13 requires exactly that once the
// catalog entries land, because pnpm catalog resolution is what CI uses. Deleted in Task 4, once
// the real suites import the same symbols and prove the same thing by using them.
import { describe, expect, it } from 'vitest';
import { createClient, createContextKey, createContextValues } from '@connectrpc/connect';
import { createGrpcTransport, Http2SessionManager } from '@connectrpc/connect-node';
import { TenancyService } from '@paigasus/proto/iam';

describe('M2 — connect 2.2.0 context-value API resolves from the workspace', () => {
  it('exports createContextKey and createContextValues', () => {
    expect(typeof createContextKey).toBe('function');
    expect(typeof createContextValues).toBe('function');
  });

  it('round-trips a value through a context key', () => {
    const key = createContextKey<string>('default');
    expect(createContextValues().get(key)).toBe('default');
    expect(createContextValues().set(key, 'set').get(key)).toBe('set');
  });
});

describe('M3 — the node transport and the typed client resolve from the workspace', () => {
  it('exports createGrpcTransport and Http2SessionManager', () => {
    expect(typeof createGrpcTransport).toBe('function');
    expect(typeof Http2SessionManager).toBe('function');
  });

  it('builds a typed client over the TenancyService descriptor', () => {
    const transport = createGrpcTransport({ baseUrl: 'https://iam.invalid' });
    const client = createClient(TenancyService, transport);
    expect(typeof client.createOrganization).toBe('function');
  });
});
