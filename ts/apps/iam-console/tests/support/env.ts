// SPDX-License-Identifier: Apache-2.0
//
// A complete, valid environment for lib/config.ts, and a helper that installs it with vi.stubEnv.
// vitest.config.ts already sets PAIGASUS_COMPILED_ZONE/BASE_PATH, the two values next.config.ts
// compiles into a real build.
import { vi } from 'vitest';

export const VALID_ENV: Readonly<Record<string, string>> = {
  PAIGASUS_ZONE: 'iam',
  PAIGASUS_ZONES: '{"iam":"/iam"}',
  PAIGASUS_OIDC_ISSUER: 'https://idp.example.test',
  PAIGASUS_OIDC_CLIENT_ID: 'console',
  PAIGASUS_OIDC_CLIENT_SECRET: 'console-secret',
  PAIGASUS_PUBLIC_ORIGIN: 'https://console.example.test',
  PAIGASUS_SESSION_STORE: 'memory',
  PAIGASUS_SERVICES: '{"iam":"http://iam.internal:8080"}',
  PAIGASUS_IAM_GRPC_URL: 'http://iam.internal:9090',
};

/** Stubs VALID_ENV plus `overrides`; an `undefined` override removes the variable. Undo with vi.unstubAllEnvs(). */
export function stubConsoleEnv(overrides: Record<string, string | undefined> = {}): void {
  for (const [key, value] of Object.entries({ ...VALID_ENV, ...overrides })) {
    vi.stubEnv(key, value);
  }
}
