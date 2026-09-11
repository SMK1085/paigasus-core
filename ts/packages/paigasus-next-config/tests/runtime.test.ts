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

/** The message of the error `fn` throws. Fails the test when `fn` does not throw. */
function thrownMessage(fn: () => unknown): string {
  try {
    fn();
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }
  throw new Error('expected the call to throw');
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
    // Spec § 9.1: the public copy is a PLAIN object, and object spread keeps the own key.
    expect(Object.getPrototypeOf(zones)).toBe(Object.prototype);
    // And it must not have polluted anything on the way through.
    expect(Object.getPrototypeOf({})).toBe(Object.prototype);
    expect(({} as Record<string, unknown>)['/evil']).toBeUndefined();
  });

  it('resolves a missing zone id as absent in the INTERNAL map, even when it names an Object.prototype member', () => {
    setEnv({ PAIGASUS_ZONES: '{"iam":"/iam"}' });
    // The internal map, which assertCompiledAgreement reads. It keeps its null prototype (spec § 9.1).
    // SMA-510 retargeted this row: it used to read getPublicConfig().zones, which is now a plain object.
    const zones = defineRuntimeConfig().getRuntimeConfig().PAIGASUS_ZONES;
    // Indexed through a VARIABLE key, exactly as assertCompiledAgreement does. A literal key would
    // make TypeScript resolve `toString` to Object.prototype's declared method rather than to the
    // index signature, which both changes the type and trips @typescript-eslint/unbound-method.
    const lookup = (id: string): string | undefined => zones[id];
    expect(lookup('toString')).toBeUndefined();
    expect(lookup('constructor')).toBeUndefined();
    expect(lookup('hasOwnProperty')).toBeUndefined();
  });

  it('the PUBLIC copy answers membership only through Object.hasOwn', () => {
    setEnv({ PAIGASUS_ZONES: '{"iam":"/iam"}' });
    const { zones } = defineRuntimeConfig().getPublicConfig();
    // A plain object inherits `toString` and `constructor`. Every consumer must test membership with
    // Object.hasOwn — @paigasus/app-shell does (spec § 6.1).
    expect(Object.hasOwn(zones, 'iam')).toBe(true);
    expect(Object.hasOwn(zones, 'toString')).toBe(false);
    expect(Object.hasOwn(zones, 'constructor')).toBe(false);
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

  it('returns the public zone map as a PLAIN object, which React Flight can pass as a prop', () => {
    // Spec F17: React Flight throws for a null-prototype prop ("Classes or null prototypes are not
    // supported"), and ZoneProvider receives this map as a prop from a server layout.
    const { zones } = defineRuntimeConfig().getPublicConfig();
    expect(Object.getPrototypeOf(zones)).toBe(Object.prototype);
    expect(zones).toEqual({ iam: '/iam', gateway: '/gateway' });
  });

  it('rejects two zones on one base path, naming both zone ids and withholding the value', () => {
    // "/shared-prefix/" canonicalises to "/shared-prefix", so the comparison must run on canonical values.
    setEnv({ PAIGASUS_ZONES: '{"iam":"/shared-prefix","gateway":"/shared-prefix/"}' });
    const message = thrownMessage(() => defineRuntimeConfig().getRuntimeConfig());
    expect(message).toMatch(/entries "iam" and "gateway" declare the same base path/);
    expect(message).not.toContain('shared-prefix');
  });

  it('rejects two root-mounted zones', () => {
    setEnv({ PAIGASUS_ZONES: '{"a":"","b":"/"}' });
    expect(() => defineRuntimeConfig().getRuntimeConfig()).toThrow(/entries "a" and "b" declare the same base path/);
  });
});
