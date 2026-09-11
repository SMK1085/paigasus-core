// SPDX-License-Identifier: Apache-2.0
//
// lib/config.ts (spec § 4.1, § 9.2). defineRuntimeConfig memoizes a SUCCESSFUL parse for the life
// of the module, so each case imports a fresh copy after vi.resetModules().
import { afterEach, describe, expect, it, vi } from 'vitest';
import { stubConsoleEnv } from '../support/env';

async function load(overrides: Record<string, string | undefined>) {
  vi.resetModules();
  stubConsoleEnv(overrides);
  return import('../../lib/config');
}

afterEach(() => {
  vi.unstubAllEnvs();
});

describe('getRuntimeConfig', () => {
  it('parses a complete environment and canonicalizes the IAM gRPC address', async () => {
    const { getRuntimeConfig } = await load({ PAIGASUS_IAM_GRPC_URL: 'http://iam.internal:9090/' });
    const config = getRuntimeConfig();
    expect(config.PAIGASUS_IAM_GRPC_URL).toBe('http://iam.internal:9090');
    expect(config.PAIGASUS_SERVICES['iam']).toBe('http://iam.internal:8080');
    expect(config.PAIGASUS_OIDC_ISSUER).toBe('https://idp.example.test');
  });

  // 'not a url' is the value that reaches the `URL.canParse` branch. 'iam.internal:9090' does not:
  // it IS a URL, with the scheme `iam.internal:` (MEASURED), so the scheme rule refuses it instead.
  // Without the canParse guard, `new URL('not a url')` throws a TypeError ("Invalid URL") out of
  // the zod parse (MEASURED, zod 4.5.4). It names no key, so this case fails its first assertion.
  it.each([
    ['not a URL', 'not a url'],
    ['a non-http scheme', 'grpc://iam.internal:9090'],
    ['credentials', 'http://user:pass@iam.internal:9090'],
    ['a query', 'http://iam.internal:9090?x=1'],
    ['a fragment', 'http://iam.internal:9090#x'],
  ])('refuses PAIGASUS_IAM_GRPC_URL with %s, and never echoes the value', async (_label, value) => {
    const { getRuntimeConfig } = await load({ PAIGASUS_IAM_GRPC_URL: value });
    let message = '';
    try {
      getRuntimeConfig();
    } catch (err) {
      message = err instanceof Error ? err.message : String(err);
    }
    expect(message).toMatch(/PAIGASUS_IAM_GRPC_URL/);
    expect(message).not.toContain(value);
  });

  it('refuses a missing PAIGASUS_IAM_GRPC_URL', async () => {
    const { getRuntimeConfig } = await load({ PAIGASUS_IAM_GRPC_URL: undefined });
    expect(() => getRuntimeConfig()).toThrow(/PAIGASUS_IAM_GRPC_URL/);
  });

  it('refuses a PAIGASUS_SERVICES map without an iam entry', async () => {
    const { getRuntimeConfig } = await load({ PAIGASUS_SERVICES: '{"gateway":"http://gateway.internal:8080"}' });
    expect(() => getRuntimeConfig()).toThrow(/PAIGASUS_SERVICES/);
  });

  it('still applies discovery’s own service-map rules', async () => {
    const { getRuntimeConfig } = await load({ PAIGASUS_SERVICES: '{"iam":"http://iam.internal:8080","unknown":"http://x.internal"}' });
    expect(() => getRuntimeConfig()).toThrow(/PAIGASUS_SERVICES/);
  });

  it('projects only the zone and the zone map into the public config', async () => {
    const { getPublicConfig } = await load({});
    expect(getPublicConfig()).toEqual({ zone: 'iam', zones: { iam: '/iam' } });
  });
});
