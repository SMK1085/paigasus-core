// SPDX-License-Identifier: Apache-2.0
//
// A runtime self-signed certificate for Keycloak's HTTPS listener, mirroring
// rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs (which uses `rcgen` — no TS equivalent is
// in the workspace catalog, so this shells out to `openssl`, present on every CI runner and every
// engineer's machine this repo already assumes has Docker).
//
// GENERATED ONCE, AT A FIXED PATH, BEFORE globalSetup RUNS. playwright.config.ts calls
// `ensureKeycloakCert()` at MODULE-EVALUATION time (before `defineConfig(...)` is even
// constructed) so the cert file's PATH is stable before `webServer.env.NODE_EXTRA_CA_CERTS` is
// read — Node reads that variable once at process bootstrap, so it cannot be set after the
// fixture server process has already started. tests/e2e/global-setup.ts calls the same function
// afterwards; `existsSync` makes the second call a no-op, so there is exactly one generation per
// `playwright test` invocation, done by whichever of the two runs first (always the config, per
// Playwright's own load order).
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const CERT_DIR = path.join(os.tmpdir(), 'paigasus-auth-e2e-tls');
export const CERT_PATH = path.join(CERT_DIR, 'keycloak-cert.pem');
export const KEY_PATH = path.join(CERT_DIR, 'keycloak-key.pem');

export interface KeycloakCert {
  certPath: string;
  keyPath: string;
}

export function ensureKeycloakCert(): KeycloakCert {
  mkdirSync(CERT_DIR, { recursive: true });
  if (!existsSync(CERT_PATH) || !existsSync(KEY_PATH)) {
    execFileSync('openssl', [
      'req',
      '-x509',
      '-newkey',
      'rsa:2048',
      '-nodes',
      '-keyout',
      KEY_PATH,
      '-out',
      CERT_PATH,
      '-days',
      '1',
      '-subj',
      '/CN=127.0.0.1',
      '-addext',
      'subjectAltName=DNS:localhost,IP:127.0.0.1',
    ]);
  }
  return { certPath: CERT_PATH, keyPath: KEY_PATH };
}
