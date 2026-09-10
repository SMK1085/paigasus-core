// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { z } from 'zod';
import { discoveryEnvShape, parseServiceMap, timingsFromEnv } from '../src/config.js';
import { DEFAULT_TIMINGS } from '../src/core/record.js';

describe('parseServiceMap', () => {
  it('accepts a well-formed map', () => {
    const map = parseServiceMap('{"iam":"http://iam:8080","gateway":"https://gw.internal"}');
    expect(map).toEqual({ iam: 'http://iam:8080', gateway: 'https://gw.internal' });
  });

  it('accepts an empty map', () => {
    expect(parseServiceMap('{}')).toEqual({});
  });

  it('rejects an unknown service key', () => {
    // An operator typo would otherwise silently empty the console: the key never matches a
    // capability's first dot-segment, every service reads `absent`, and nothing renders.
    expect(() => parseServiceMap('{"iamm":"http://iam:8080"}')).toThrow(/iamm/);
  });

  it('rejects a key that is not lowercase', () => {
    expect(() => parseServiceMap('{"IAM":"http://iam:8080"}')).toThrow(/IAM/);
  });

  it.each(['ftp://iam:8080', 'file:///etc/passwd', 'not-a-url'])('rejects the scheme in %s', (url) => {
    expect(() => parseServiceMap(JSON.stringify({ iam: url }))).toThrow();
  });

  it('rejects userinfo in the URL', () => {
    expect(() => parseServiceMap('{"iam":"http://user:pass@iam:8080"}')).toThrow();
  });

  it('rejects a query or fragment', () => {
    expect(() => parseServiceMap('{"iam":"http://iam:8080/?a=1"}')).toThrow();
    expect(() => parseServiceMap('{"iam":"http://iam:8080/#x"}')).toThrow();
  });

  it('strips a trailing slash so the probe path never doubles it', () => {
    expect(parseServiceMap('{"iam":"http://iam:8080/"}')).toEqual({ iam: 'http://iam:8080' });
  });

  it('rejects a non-object', () => {
    expect(() => parseServiceMap('[]')).toThrow();
    expect(() => parseServiceMap('"iam"')).toThrow();
  });

  it('rejects unparseable JSON', () => {
    expect(() => parseServiceMap('{oops')).toThrow();
  });

  it('produces a null-prototype object', () => {
    // Same prototype-pollution rule @paigasus/next-config applies to PAIGASUS_ZONES.
    expect(Object.getPrototypeOf(parseServiceMap('{"iam":"http://iam:8080"}'))).toBeNull();
  });
});

describe('timingsFromEnv', () => {
  it('maps each PAIGASUS_DISCOVERY_* key onto its matching Timings field', () => {
    const config = z.object(discoveryEnvShape).parse({
      PAIGASUS_SERVICES: '{}',
      PAIGASUS_DISCOVERY_NEGATIVE_MS: '5000',
      PAIGASUS_DISCOVERY_FRESH_MS: '30000',
      PAIGASUS_DISCOVERY_STALE_MS: '300000',
      PAIGASUS_DISCOVERY_PROBE_TIMEOUT_MS: '1000',
      PAIGASUS_DISCOVERY_LOCK_WAIT_MS: '2000',
      PAIGASUS_DISCOVERY_LOCK_TTL_MS: '4000',
    });
    expect(timingsFromEnv(config)).toEqual({
      negativeMs: 5000,
      freshMs: 30000,
      staleMs: 300000,
      probeTimeoutMs: 1000,
      lockWaitMs: 2000,
      lockTtlMs: 4000,
    });
  });

  it('falls back to DEFAULT_TIMINGS when the env vars are unset, so the result stays valid', () => {
    const config = z.object(discoveryEnvShape).parse({ PAIGASUS_SERVICES: '{}' });
    expect(timingsFromEnv(config)).toEqual(DEFAULT_TIMINGS);
  });
});
