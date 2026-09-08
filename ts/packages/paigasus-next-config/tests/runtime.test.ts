// SPDX-License-Identifier: Apache-2.0
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { z } from 'zod';
import { defineRuntimeConfig, PUBLIC_CONFIG_KEYS } from '../src/runtime.js';

const ORIGINAL = { ...process.env };

function setEnv(overrides: Record<string, string | undefined>) {
  for (const [key, value] of Object.entries(overrides)) {
    if (value === undefined) delete process.env[key];
    else process.env[key] = value;
  }
}

beforeEach(() => {
  for (const key of Object.keys(process.env)) {
    if (key.startsWith('PAIGASUS_') || key === 'NEXT_PHASE' || key === 'SECRET_TOKEN') delete process.env[key];
  }
  setEnv({
    PAIGASUS_ZONE: 'iam',
    PAIGASUS_ZONES: JSON.stringify({ iam: '/iam', gateway: '/gateway' }),
    PAIGASUS_COMPILED_ZONE: 'iam',
    PAIGASUS_COMPILED_BASE_PATH: '/iam',
  });
});

afterEach(() => {
  process.env = { ...ORIGINAL };
});

describe('defineRuntimeConfig', () => {
  it('parses a valid environment', () => {
    const { getRuntimeConfig } = defineRuntimeConfig();
    const cfg = getRuntimeConfig();
    expect(cfg.PAIGASUS_ZONE).toBe('iam');
    expect(cfg.PAIGASUS_ZONES).toEqual({ iam: '/iam', gateway: '/gateway' });
  });

  it('names the offending variable when the environment is invalid', () => {
    setEnv({ PAIGASUS_ZONES: 'not-json' });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/PAIGASUS_ZONES/);
  });

  it('memoizes on first success — a later process.env mutation is not observed', () => {
    const { getRuntimeConfig } = defineRuntimeConfig();
    expect(getRuntimeConfig().PAIGASUS_ZONE).toBe('iam');
    setEnv({ PAIGASUS_ZONE: 'gateway', PAIGASUS_COMPILED_ZONE: 'gateway' });
    expect(getRuntimeConfig().PAIGASUS_ZONE).toBe('iam');
  });

  it('does NOT memoize a failure — a misconfigured container fails every request', () => {
    setEnv({ PAIGASUS_ZONES: 'not-json' });
    const { getRuntimeConfig } = defineRuntimeConfig();
    expect(() => getRuntimeConfig()).toThrow();
    expect(() => getRuntimeConfig()).toThrow();
  });

  it('throws during next build, so a module-scope read cannot bake in a value', () => {
    setEnv({ NEXT_PHASE: 'phase-production-build' });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/next build/);
  });

  it('fails CLOSED when the compiled zone is absent', () => {
    setEnv({ PAIGASUS_COMPILED_ZONE: undefined });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/PAIGASUS_COMPILED_ZONE/);
  });

  it('fails closed when the compiled base path is absent', () => {
    setEnv({ PAIGASUS_COMPILED_BASE_PATH: undefined });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/PAIGASUS_COMPILED_BASE_PATH/);
  });

  it('detects the image deployed as the wrong zone, naming both values', () => {
    setEnv({ PAIGASUS_ZONE: 'gateway', PAIGASUS_ZONES: JSON.stringify({ gateway: '/gateway' }) });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/iam[\s\S]*gateway|gateway[\s\S]*iam/);
  });

  it('detects an ingress remap, naming both base paths', () => {
    setEnv({ PAIGASUS_ZONES: JSON.stringify({ iam: '/admin/iam' }) });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/\/admin\/iam/);
  });

  it('treats a trailing slash in the operator JSON as equivalent', () => {
    setEnv({ PAIGASUS_ZONES: JSON.stringify({ iam: '/iam/' }) });
    expect(defineRuntimeConfig().getRuntimeConfig().PAIGASUS_ZONES.iam).toBe('/iam');
  });

  it('accepts a zone mounted at the origin root', () => {
    setEnv({ PAIGASUS_ZONES: JSON.stringify({ iam: '/' }), PAIGASUS_COMPILED_BASE_PATH: '' });
    expect(defineRuntimeConfig().getRuntimeConfig().PAIGASUS_ZONES.iam).toBe('');
  });

  it('throws when the zone map has no entry for this app zone', () => {
    setEnv({ PAIGASUS_ZONES: JSON.stringify({ gateway: '/gateway' }) });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/no entry/i);
  });

  it('rejects a non-path value inside the zone map at parse time', () => {
    setEnv({ PAIGASUS_ZONES: JSON.stringify({ iam: 'https://idp.example/secret' }) });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/PAIGASUS_ZONES/);
  });

  it('throws when an extra shape declares a key this package owns', () => {
    expect(() => defineRuntimeConfig({ PAIGASUS_ZONE: z.string() })).toThrow(/PAIGASUS_ZONE/);
  });

  it('validates an extra shape alongside the core one', () => {
    setEnv({ SECRET_TOKEN: 'sk-live-abc123' });
    const cfg = defineRuntimeConfig({ SECRET_TOKEN: z.string().min(1) }).getRuntimeConfig();
    expect(cfg.SECRET_TOKEN).toBe('sk-live-abc123');
  });

  it('keeps an extra-shape secret out of the client slice', () => {
    setEnv({ SECRET_TOKEN: 'sk-live-abc123' });
    const { getPublicConfig } = defineRuntimeConfig({ SECRET_TOKEN: z.string().min(1) });
    expect(JSON.stringify(getPublicConfig())).not.toContain('sk-live-abc123');
  });

  it('projects exactly the pinned public keys', () => {
    expect(Object.keys(defineRuntimeConfig().getPublicConfig()).sort()).toEqual([...PUBLIC_CONFIG_KEYS].sort());
  });

  it('pins the public key list by strict equality, so widening the slice is deliberate', () => {
    expect(PUBLIC_CONFIG_KEYS).toEqual(['zone', 'zones']);
  });

  it('never puts an extra-shape value into an error message', () => {
    setEnv({ SECRET_TOKEN: 'sk-live-abc123', PAIGASUS_ZONE: '' });
    const { getRuntimeConfig } = defineRuntimeConfig({ SECRET_TOKEN: z.string().min(1) });
    try {
      getRuntimeConfig();
      throw new Error('expected a validation failure');
    } catch (err) {
      expect(String(err)).not.toContain('sk-live-abc123');
      expect(String(err)).toContain('PAIGASUS_ZONE');
    }
  });
});
