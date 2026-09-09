// SPDX-License-Identifier: Apache-2.0
//
// Shared, fixed values for the containerized E2E tier (task 13). The fixture server's port is
// FIXED, not testcontainers-assigned, because the Keycloak client's `redirectUris` /
// `postLogoutRedirectUris` are baked into tests/e2e/keycloak-realm.json at commit time, imported
// into Keycloak via `--import-realm` before this file (or any config) is ever read. JSON has no
// imports, so keeping the realm fixture in sync with the values below is a hand-maintained
// invariant, not an enforced one — the comment in that file says so too.
export const FIXTURE_SERVER_PORT = 4319;

/**
 * `http://`, not `https://` — deliberately (see fixture-server.ts's own header, and M11 in
 * docs/superpowers/specs/2026-09-09-sma-506-measurements.md). This is the ONE place in the
 * codebase that constructs a `ComposedConfig` with a non-`https://` `PAIGASUS_PUBLIC_ORIGIN`:
 * `src/config.ts`'s `httpsUrl` schema would reject it, but that schema is only ever applied by
 * `@paigasus/next-config`'s `defineRuntimeConfig`, which this fixture never calls — it builds a
 * `ComposedConfig` object literally, the same way tests/runtime.test.ts does, and is not a
 * precedent for a real app to do the same.
 */
export const FIXTURE_SERVER_ORIGIN = `http://127.0.0.1:${String(FIXTURE_SERVER_PORT)}`;

export const ZONE = 'e2e';
export const ZONE_BASE_PATH = '/e2e';

export const KEYCLOAK_REALM = 'paigasus-test';
export const KEYCLOAK_HTTPS_PORT = 8443;

/**
 * Must match tests/e2e/keycloak-realm.json's second client (M6). `paigasus-cli`, the realm
 * fixture's OTHER client (shared with rs/crates/services/paigasus-iam's own E2E test), cannot be
 * reused for an authorization-code round trip: it is `publicClient: true` with
 * `standardFlowEnabled: false` (password-grant only) and no client secret. `paigasus-e2e-rp` is
 * CONFIDENTIAL (`publicClient: false`) with the standard (authorization code) flow enabled, this
 * fixed secret, and `redirectUris`/`postLogoutRedirectUris` entries matching FIXTURE_SERVER_ORIGIN
 * above. `offline_access` is promoted from Keycloak's default OPTIONAL client scope to a DEFAULT
 * one on that client so `PAIGASUS_OIDC_SCOPES`'s `openid profile email offline_access` issues a
 * refresh_token without any extra consent-flow wiring.
 *
 * The realm JSON itself carries NO explanatory comment field (JSON has none, and Keycloak's realm
 * importer rejects an unrecognised property — `_comment` on a `ClientRepresentation` was tried and
 * measured to hard-fail the import with `Unrecognized field "_comment"`, from Jackson's strict
 * deserialization) — this comment is that documentation's only home.
 */
export const KEYCLOAK_CLIENT_ID = 'paigasus-e2e-rp';
export const KEYCLOAK_CLIENT_SECRET = 'e2e-test-secret';

export const KEYCLOAK_USERNAME = 'alice';
export const KEYCLOAK_PASSWORD = 'alice-password';

/** The session cookie's name (src/http/cookies.ts's `SESSION_COOKIE`). Duplicated deliberately —
 * see roundtrip.spec.ts's own header for why the spec files treat this as an observable contract
 * rather than reaching back into the module under test. */
export const SESSION_COOKIE_NAME = '__Host-pgs_sid';
