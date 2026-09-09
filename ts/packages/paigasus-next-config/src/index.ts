// SPDX-License-Identifier: Apache-2.0
import type { NextConfig } from 'next';
import { canonicalBasePath } from './base-path';

export { canonicalBasePath } from './base-path';

/**
 * Workspace packages that export TypeScript source rather than built JS. Next must transpile
 * them. Every `@paigasus/*` package is `private: true` with `exports` pointing at `./src/*.ts`;
 * this list grows when a new one lands.
 */
const SOURCE_ONLY_PACKAGES = ['@paigasus/kernel', '@paigasus/next-config', '@paigasus/proto', '@paigasus/sdk', '@paigasus/ui'];

export interface CreateNextConfigOptions {
  /** This app's zone id, e.g. `iam`. `PAIGASUS_ZONE` must equal it at runtime. */
  zone: string;
  /** The zone's path prefix. `'/'` or `''` for a zone mounted at the origin root. */
  basePath: string;
  /** CDN offload only, and deliberately NOT defaulted — see the note below. */
  assetPrefix?: string;
  /** Absolute path Next traces the standalone output from. Pin it; the inferred value moves. */
  outputFileTracingRoot: string;
  /** Any other Next options. `env` is refused. */
  extend?: NextConfig;
}

/**
 * Build one console zone's Next configuration.
 *
 * `output: 'standalone'` is forced LAST and cannot be overridden by `extend`. Without it the
 * image ships all of `node_modules`, which defeats the self-hosting story outright.
 *
 * `assetPrefix` is NOT defaulted to `basePath`. With `basePath: '/iam'` Next already serves
 * chunks under `/iam/_next/`, so the default would buy nothing. `assetPrefix` exists for CDN
 * offload, a different axis. (The double-prefix 404 an earlier revision feared is DISPROVEN —
 * measured on Next 16.3.4, the two keys do not concatenate. Spec § 4.4, § 13 M3.)
 *
 * `env` is refused from `extend` because `env` inlines values at build time exactly as
 * `NEXT_PUBLIC_` does. The factory owns it, and writes only the two values that describe the
 * IMAGE: which zone this image is, and where it is mounted. Everything that varies per
 * DEPLOYMENT belongs in `@paigasus/next-config/runtime`.
 */
export function createNextConfig(options: CreateNextConfigOptions): NextConfig {
  const { zone, basePath, assetPrefix, outputFileTracingRoot, extend } = options;

  if (extend && Object.prototype.hasOwnProperty.call(extend, 'env')) {
    throw new Error(
      'createNextConfig: `extend.env` is not allowed. `env` inlines values at build time, so it is the same single-image hazard as NEXT_PUBLIC_. ' +
        'The factory owns `env`; deployment-varying values belong in @paigasus/next-config/runtime.',
    );
  }
  // REJECTED, NOT TRIMMED — and the difference is a whole deployment class. `zone` is written
  // verbatim into PAIGASUS_COMPILED_ZONE, and runtime.ts compares that string to `PAIGASUS_ZONE`
  // with `!==`. So `zone: ' iam '` used to pass this check (it is not whitespace-ONLY), compile as
  // ' iam ', and then fail every request of a CORRECTLY configured deployment that sets
  // PAIGASUS_ZONE=iam — with a mismatch message whose two sides look identical in a log.
  //
  // Trimming would hide that instead of fixing it. A zone id is also a KEY in the PAIGASUS_ZONES
  // map that Helm generates, and nothing trims those keys, so accepting ' iam ' here would make
  // ' iam ' and 'iam' two spellings of one zone that agree in the image and disagree in the map.
  // One spelling, refused loudly at build time, is the only version with no silent half.
  if (zone !== zone.trim() || zone === '') {
    throw new Error('createNextConfig: `zone` must be a non-empty zone id with no leading or trailing whitespace');
  }

  const canonical = canonicalBasePath(basePath, 'createNextConfig: basePath');
  const transpilePackages = [...new Set([...SOURCE_ONLY_PACKAGES, ...(extend?.transpilePackages ?? [])])];

  return {
    ...extend,
    // `exactOptionalPropertyTypes` is on, so an optional key is omitted by conditional spread
    // rather than set to `undefined`.
    ...(canonical === '' ? {} : { basePath: canonical }),
    ...(assetPrefix === undefined ? {} : { assetPrefix }),
    outputFileTracingRoot,
    transpilePackages,
    env: {
      PAIGASUS_COMPILED_ZONE: zone,
      PAIGASUS_COMPILED_BASE_PATH: canonical,
    },
    output: 'standalone',
  };
}
