// SPDX-License-Identifier: Apache-2.0
//
// The environment the dev stack (SMA-641) gives both `next dev` children. PURE: it takes the
// running fakes' addresses and returns a record, so a unit test holds the whole contract. The
// supervisor in ts/tooling/dev-stack.ts has no vitest project of its own, which is why this lives
// here rather than beside it.
//
// It mirrors ts/apps/gateway-console/tests/e2e/support/two-zone-harness.ts's `serverEnv` with ONE
// deliberate difference: it does NOT set PAIGASUS_DISCOVERY_NEGATIVE_MS/_FRESH_MS/_STALE_MS. The
// e2e tier sets those to 1, 2 and 3 milliseconds to force a re-probe on every call. A dev server
// must behave the way a deployment does, so it uses the real defaults.

/** The zone map both children share. BYTE-IDENTICAL in both, or the compiled-agreement check fails. */
export const DEV_ZONES = JSON.stringify({ iam: '/iam', gateway: '/gateway' });

export type DevEnvInput = {
  /** Usually `process.env`. Its PAIGASUS_*, __NEXT* and NODE_ENV keys are removed. */
  readonly parentEnv: NodeJS.ProcessEnv;
  readonly certPath: string;
  readonly idp: { readonly issuer: string; readonly clientId: string; readonly clientSecret: string };
  readonly iam: { readonly grpcUrl: string; readonly httpUrl: string };
  readonly gatewayUrl: string;
  readonly redisUrl: string;
  readonly publicOrigin: string;
};

/**
 * The shared environment. Each child adds PORT, HOSTNAME and its own PAIGASUS_ZONE.
 *
 * HOSTNAME is load-bearing rather than cosmetic: Next's dev server builds its cross-site
 * allowlist from the BIND hostname (`block-cross-site-dev.js`), so `127.0.0.1` is an accepted
 * Origin only because the child binds it. Dropping HOSTNAME produces a 403 that talks about
 * `allowedDevOrigins` and never mentions the bind host.
 */
export function buildDevEnv(input: DevEnvInput): Record<string, string> {
  const base: Record<string, string> = {};
  for (const [key, value] of Object.entries(input.parentEnv)) {
    if (value === undefined) continue;
    // The developer's own shell or .env.local must not reach the child: the stack owns this
    // configuration completely, exactly as the e2e harness owns the standalone server's.
    if (key === 'NODE_ENV' || key.startsWith('PAIGASUS_') || key.startsWith('__NEXT')) continue;
    base[key] = value;
  }
  return {
    ...base,
    NEXT_TELEMETRY_DISABLED: '1',
    NODE_EXTRA_CA_CERTS: input.certPath,
    PAIGASUS_ZONES: DEV_ZONES,
    PAIGASUS_OIDC_ISSUER: input.idp.issuer,
    PAIGASUS_OIDC_CLIENT_ID: input.idp.clientId,
    PAIGASUS_OIDC_CLIENT_SECRET: input.idp.clientSecret,
    PAIGASUS_PUBLIC_ORIGIN: input.publicOrigin,
    // MANDATORY, not a preference: createAuthRuntime throws on the memory store when
    // PAIGASUS_ZONES names more than one zone (paigasus-auth/src/runtime.ts:105-113).
    PAIGASUS_SESSION_STORE: 'redis',
    PAIGASUS_SESSION_REDIS_URL: input.redisUrl,
    PAIGASUS_SERVICES: JSON.stringify({ iam: input.iam.httpUrl, gateway: input.gatewayUrl }),
    PAIGASUS_IAM_GRPC_URL: input.iam.grpcUrl,
  };
}
