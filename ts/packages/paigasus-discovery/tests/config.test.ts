// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { parseServiceMap } from '../src/config.js';

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
