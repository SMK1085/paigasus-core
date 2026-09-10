// SPDX-License-Identifier: Apache-2.0
//
// A runtime self-signed certificate for Keycloak's HTTPS listener, mirroring
// rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs (which uses `rcgen` — no TS equivalent is
// in the workspace catalog, so this shells out to `openssl`, present on every CI runner and every
// engineer's machine this repo already assumes has Docker).
//
// GENERATED AT A FIXED PATH, ON WHICHEVER OF TWO PATHS REACHES IT FIRST (fix round 1, m4 —
// corrected from an earlier, inaccurate version of this comment). The cert file's PATH must be
// stable before `webServer.env.NODE_EXTRA_CA_CERTS` is read in `playwright.config.ts`, because
// Node reads that variable once at process bootstrap and cannot pick it up if it is set only
// after the fixture server process has already started. `playwright.config.ts` itself does NOT
// call `ensureKeycloakCert()` directly — it calls `startE2eInfrastructure()`
// (`global-setup.ts`), which reaches this function only via `startContainers()`, and NOT AT ALL
// on the "adopt an already-published environment" path (`adoptOrStartContainers()`) a worker
// process takes once the main/runner process has already started everything. Either way, the
// PATH constants below (`CERT_PATH`/`KEY_PATH`) are fixed and computed at import time regardless
// of which branch runs, which is what keeps the `NODE_EXTRA_CA_CERTS` value stable across both.
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

/**
 * m2 (fix round 1): a plain `existsSync` check is not enough. The cert is minted with `-days 1`
 * at a FIXED path under `os.tmpdir()` — on a machine where that directory survives past one day
 * (a long-lived dev box or a persistent CI cache, unlike a fresh container per run), a bare
 * existence check would reuse an already-expired cert on day two and fail with an opaque TLS
 * error deep inside Keycloak's TLS handshake, nothing like "the cert expired". `-checkend 300`
 * additionally requires at least 5 more minutes of validity — long enough to outlive one test
 * run — before treating the existing pair as reusable.
 */
function certStillValid(): boolean {
  if (!existsSync(CERT_PATH) || !existsSync(KEY_PATH)) return false;
  try {
    execFileSync('openssl', ['x509', '-in', CERT_PATH, '-checkend', '300', '-noout']);
    return true;
  } catch {
    return false;
  }
}

export function ensureKeycloakCert(): KeycloakCert {
  mkdirSync(CERT_DIR, { recursive: true });
  if (!certStillValid()) {
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
