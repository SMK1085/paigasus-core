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
    if (key.startsWith('PAIGASUS_') || key === 'NEXT_PHASE' || key === 'NEXT_RUNTIME' || key === 'SECRET_TOKEN') delete process.env[key];
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

  // `server-only` is a CLIENT-bundle guard only: Next sets the `react-server` condition for the
  // middleware layer, so that import resolves to server-only's empty.js there and a middleware
  // importing this module builds cleanly (MEASURED — measurements document M6). This branch is
  // what actually closes the edge door.
  it('throws in the edge runtime, where process.env is not dynamic', () => {
    setEnv({ NEXT_RUNTIME: 'edge' });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/EDGE runtime/);
  });

  it('does not throw in the node runtime', () => {
    setEnv({ NEXT_RUNTIME: 'nodejs' });
    expect(defineRuntimeConfig().getRuntimeConfig().PAIGASUS_ZONE).toBe('iam');
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

  // The extra-shape leak this file's owner check closes. A `.refine()` supplied by @paigasus/auth
  // or @paigasus/sdk raises code 'custom', and its message may interpolate the input. Rendering
  // any custom message verbatim would therefore put a secret into the thrown error and the
  // container log as soon as the first extra shape ships one.
  //
  // The `{ error: (iss) => … }` form is deliberate and MEASURED: on zod 4.5.4 the zod-3
  // `.refine(fn, (v) => ({ message }))` function form is IGNORED and yields the generic
  // 'Invalid input', so a test written that way would pass with this guard deleted. This form
  // renders the input verbatim, so it is the one that actually exercises the guard.
  it('never renders a custom message authored by an extra shape, even though its code is custom', () => {
    setEnv({ SECRET_TOKEN: 'sk-live-abc123' });
    const shape = {
      SECRET_TOKEN: z.string().refine(() => false, { error: (iss: { input: unknown }) => `${String(iss.input)} is not a valid issuer` }),
    };
    const { getRuntimeConfig } = defineRuntimeConfig(shape);
    try {
      getRuntimeConfig();
      throw new Error('expected a validation failure');
    } catch (err) {
      expect(String(err)).not.toContain('sk-live-abc123');
      expect(String(err)).toContain('SECRET_TOKEN');
    }
  });

  it('still renders this package OWN custom messages, which are authored here and value-free', () => {
    setEnv({ PAIGASUS_ZONES: 'not-json' });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/is not valid JSON/);
  });

  it('names the secret variable but never its value when the secret itself fails validation', () => {
    setEnv({ SECRET_TOKEN: 'sk-live-x' });
    const { getRuntimeConfig } = defineRuntimeConfig({ SECRET_TOKEN: z.string().min(20) });
    try {
      getRuntimeConfig();
      throw new Error('expected a validation failure');
    } catch (err) {
      expect(String(err)).not.toContain('sk-live-x');
      expect(String(err)).toContain('SECRET_TOKEN');
    }
  });

  // ── prototype pollution and prototype LEAKAGE in the zone map ──
  //
  // The map was built on `{}`, which broke in two directions at once. See runtime.ts's comment on
  // `Object.create(null)` for the mechanism of each.

  it('preserves a __proto__ zone id instead of silently dropping it', () => {
    setEnv({ PAIGASUS_ZONES: '{"iam":"/iam","__proto__":"/evil"}' });
    const { zones } = defineRuntimeConfig().getPublicConfig();
    // On an object literal this assignment hits the inherited setter, is ignored, and the entry
    // vanishes from the public projection with no error at all.
    expect(Object.prototype.hasOwnProperty.call(zones, '__proto__')).toBe(true);
    expect(zones['__proto__']).toBe('/evil');
    // And it must not have polluted anything on the way through.
    expect(Object.getPrototypeOf({})).toBe(Object.prototype);
    expect(({} as Record<string, unknown>)['/evil']).toBeUndefined();
  });

  it('resolves a missing zone id as absent even when it names an Object.prototype member', () => {
    setEnv({ PAIGASUS_ZONES: '{"iam":"/iam"}' });
    const { zones } = defineRuntimeConfig().getPublicConfig();
    // Indexed through a VARIABLE key, exactly as assertCompiledAgreement does. A literal key would
    // make TypeScript resolve `toString` to Object.prototype's declared method rather than to the
    // index signature, which both changes the type and trips @typescript-eslint/unbound-method —
    // and would assert something other than the runtime lookup this test is about.
    const lookup = (id: string): string | undefined => zones[id];
    // On an object literal each of these is an inherited FUNCTION, so `zones[zone] === undefined`
    // reads as "declared" and assertCompiledAgreement skips the base-path cross-check.
    expect(lookup('toString')).toBeUndefined();
    expect(lookup('constructor')).toBeUndefined();
    expect(lookup('hasOwnProperty')).toBeUndefined();
  });

  it("fails closed when the app's own zone id only resolves through the prototype", () => {
    setEnv({ PAIGASUS_ZONE: 'toString', PAIGASUS_COMPILED_ZONE: 'toString', PAIGASUS_ZONES: '{"iam":"/iam"}' });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/no entry for this app/);
  });

  // The runtime half of createNextConfig's build-time rule. Without it a padded PAIGASUS_ZONE is
  // reported as a zone MISMATCH, whose two sides render identically in a container log.
  it('rejects a PAIGASUS_ZONE with surrounding whitespace, naming the variable', () => {
    setEnv({ PAIGASUS_ZONE: ' iam ' });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/PAIGASUS_ZONE/);
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/whitespace/);
  });

  it('rejects a padded zone id in the zone map, rather than reporting it as missing', () => {
    setEnv({ PAIGASUS_ZONES: '{" iam ":"/iam"}' });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/whitespace/);
  });
});
