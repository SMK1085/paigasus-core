// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { buildDevEnv, DEV_ZONES } from '../../testing/index';

const input = {
  parentEnv: {} as NodeJS.ProcessEnv,
  certPath: '/tmp/pair/cert.pem',
  idp: { issuer: 'https://127.0.0.1:8444', clientId: 'client', clientSecret: 'secret' },
  iam: { grpcUrl: 'http://127.0.0.1:40001', httpUrl: 'http://127.0.0.1:40002' },
  gatewayUrl: 'http://127.0.0.1:40003',
  redisUrl: 'redis://127.0.0.1:40004/0',
  publicOrigin: 'https://127.0.0.1:8443',
};

describe('buildDevEnv', () => {
  it('sets every key the two console apps have no default for', () => {
    const env = buildDevEnv(input);
    expect(env).toMatchObject({
      PAIGASUS_ZONES: '{"iam":"/iam","gateway":"/gateway"}',
      PAIGASUS_OIDC_ISSUER: 'https://127.0.0.1:8444',
      PAIGASUS_OIDC_CLIENT_ID: 'client',
      PAIGASUS_OIDC_CLIENT_SECRET: 'secret',
      PAIGASUS_PUBLIC_ORIGIN: 'https://127.0.0.1:8443',
      PAIGASUS_SESSION_STORE: 'redis',
      PAIGASUS_SESSION_REDIS_URL: 'redis://127.0.0.1:40004/0',
      PAIGASUS_SERVICES: '{"iam":"http://127.0.0.1:40002","gateway":"http://127.0.0.1:40003"}',
      PAIGASUS_IAM_GRPC_URL: 'http://127.0.0.1:40001',
      NODE_EXTRA_CA_CERTS: '/tmp/pair/cert.pem',
      NEXT_TELEMETRY_DISABLED: '1',
    });
  });

  it('does NOT set PAIGASUS_ZONE — that is per child', () => {
    expect(buildDevEnv(input).PAIGASUS_ZONE).toBeUndefined();
  });

  it('removes the parent PAIGASUS_, __NEXT and NODE_ENV keys', () => {
    const env = buildDevEnv({
      ...input,
      parentEnv: { PAIGASUS_SESSION_STORE: 'memory', PAIGASUS_ANYTHING: 'x', __NEXT_PRIVATE_ORIGIN: 'y', NODE_ENV: 'production', HOME: '/home/dev' },
    });
    expect(env.PAIGASUS_ANYTHING).toBeUndefined();
    expect(env.__NEXT_PRIVATE_ORIGIN).toBeUndefined();
    expect(env.NODE_ENV).toBeUndefined();
    // The stack's own value wins over the parent's.
    expect(env.PAIGASUS_SESSION_STORE).toEqual('redis');
    // Everything else survives: the child still needs PATH, HOME and the rest.
    expect(env.HOME).toEqual('/home/dev');
  });

  it('makes its own NODE_EXTRA_CA_CERTS and NEXT_TELEMETRY_DISABLED win over the parent env', () => {
    const env = buildDevEnv({
      ...input,
      parentEnv: { NODE_EXTRA_CA_CERTS: '/etc/ssl/wrong-cert.pem', NEXT_TELEMETRY_DISABLED: '0' },
    });
    // Neither key starts with PAIGASUS_/NODE_ENV/__NEXT, so the strip filter never removes them —
    // only spread order decides the winner. A developer with NODE_EXTRA_CA_CERTS already exported
    // for their own use must still get the stack's cert, or the child fails to trust the fake IdP.
    expect(env.NODE_EXTRA_CA_CERTS).toEqual('/tmp/pair/cert.pem');
    expect(env.NEXT_TELEMETRY_DISABLED).toEqual('1');
  });

  it('leaves the three discovery timing keys UNSET, so dev uses the real defaults', () => {
    const env = buildDevEnv(input);
    expect(env.PAIGASUS_DISCOVERY_NEGATIVE_MS).toBeUndefined();
    expect(env.PAIGASUS_DISCOVERY_FRESH_MS).toBeUndefined();
    expect(env.PAIGASUS_DISCOVERY_STALE_MS).toBeUndefined();
  });

  it('exports the zone map both children share', () => {
    expect(DEV_ZONES).toEqual('{"iam":"/iam","gateway":"/gateway"}');
  });
});
