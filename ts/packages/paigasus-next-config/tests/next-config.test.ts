// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { createNextConfig } from '../src/index.js';

const base = { zone: 'iam', basePath: '/iam', outputFileTracingRoot: '/repo/ts' } as const;

describe('createNextConfig', () => {
  it("forces output: 'standalone'", () => {
    expect(createNextConfig(base).output).toBe('standalone');
  });

  it('does not let extend override the standalone rule', () => {
    expect(createNextConfig({ ...base, extend: { output: 'export' } }).output).toBe('standalone');
  });

  it('rejects an extend.env key, because env inlines at build time', () => {
    expect(() => createNextConfig({ ...base, extend: { env: { ISSUER: 'https://idp.example' } } })).toThrow(/extend\.env/);
  });

  it('writes the compiled zone and base path into env', () => {
    const cfg = createNextConfig(base);
    expect(cfg.env).toEqual({ PAIGASUS_COMPILED_ZONE: 'iam', PAIGASUS_COMPILED_BASE_PATH: '/iam' });
  });

  it('canonicalises the base path on both the config and the compiled env value', () => {
    const cfg = createNextConfig({ ...base, basePath: 'iam/' });
    expect(cfg.basePath).toBe('/iam');
    expect(cfg.env?.PAIGASUS_COMPILED_BASE_PATH).toBe('/iam');
  });

  it("omits basePath entirely for a root-mounted zone, because Next rejects '/'", () => {
    const cfg = createNextConfig({ ...base, basePath: '/' });
    expect('basePath' in cfg).toBe(false);
    expect(cfg.env?.PAIGASUS_COMPILED_BASE_PATH).toBe('');
  });

  it('does NOT default assetPrefix — basePath already namespaces chunks', () => {
    expect('assetPrefix' in createNextConfig(base)).toBe(false);
  });

  it('passes assetPrefix through when the caller supplies one', () => {
    expect(createNextConfig({ ...base, assetPrefix: 'https://cdn.example' }).assetPrefix).toBe('https://cdn.example');
  });

  it('sets outputFileTracingRoot so the standalone entry point is predictable', () => {
    expect(createNextConfig(base).outputFileTracingRoot).toBe('/repo/ts');
  });

  it('transpiles the source-only workspace packages and unions the caller list', () => {
    const cfg = createNextConfig({ ...base, extend: { transpilePackages: ['@acme/x'] } });
    expect(cfg.transpilePackages).toContain('@paigasus/next-config');
    expect(cfg.transpilePackages).toContain('@acme/x');
  });

  it('preserves other extend keys', () => {
    expect(createNextConfig({ ...base, extend: { poweredByHeader: false } }).poweredByHeader).toBe(false);
  });

  it('rejects an empty or whitespace-only zone', () => {
    expect(() => createNextConfig({ ...base, zone: '' })).toThrow(/zone/);
    expect(() => createNextConfig({ ...base, zone: '   ' })).toThrow(/zone/);
  });

  // The padded zone is NOT the whitespace-only case, and it used to pass. `zone: ' iam '` is not
  // whitespace-only, so the old `zone.trim() === ''` check accepted it and compiled ' iam ' into
  // PAIGASUS_COMPILED_ZONE. A deployment that then set PAIGASUS_ZONE=iam — the correct value —
  // failed every request on a mismatch whose two sides look identical in a log.
  it('rejects a zone with leading or trailing whitespace, rather than trimming it', () => {
    expect(() => createNextConfig({ ...base, zone: ' iam ' })).toThrow(/whitespace/);
    expect(() => createNextConfig({ ...base, zone: 'iam ' })).toThrow(/whitespace/);
    expect(() => createNextConfig({ ...base, zone: '\tiam' })).toThrow(/whitespace/);
  });

  // The other half of that decision: refusing must not turn into trimming later. If a future
  // revision trims instead, this assertion fails rather than the one above.
  it('does not silently trim — a padded zone never reaches the compiled env', () => {
    expect(() => createNextConfig({ ...base, zone: ' iam ' })).toThrow();
    expect(createNextConfig({ ...base, zone: 'iam' }).env?.PAIGASUS_COMPILED_ZONE).toBe('iam');
  });
});
