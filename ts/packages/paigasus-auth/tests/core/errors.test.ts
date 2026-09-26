// SPDX-License-Identifier: Apache-2.0
//
// SMA-653 D2. Next 16 loads a route handler and a page with SEPARATE copies of this package, and
// the runtime (with its store) is shared between them through globalThis (src/runtime.ts). So a
// store error can be an instance of the OTHER copy's class, and `instanceof` is then false. These
// tests build that second copy with vi.resetModules() and a dynamic import.
import { describe, expect, it, vi } from 'vitest';
import {
  AuthError,
  CallbackRejected,
  OIDC_DISCOVERY_FAILURE_REASONS,
  OidcDiscoveryFailed,
  RefreshRejected,
  SessionStoreTimeout,
  SessionStoreUnavailable,
  isOidcDiscoveryFailed,
  isRefreshRejected,
  isSessionStoreUnavailable,
  oidcDiscoveryReason,
} from '../../src/core/errors.js';

describe('isSessionStoreUnavailable (SMA-653 D2)', () => {
  it('is true for a SessionStoreUnavailable and for its SessionStoreTimeout subclass', () => {
    expect(isSessionStoreUnavailable(new SessionStoreUnavailable('down'))).toBe(true);
    expect(isSessionStoreUnavailable(new SessionStoreTimeout('get', 4000, 'deadline'))).toBe(true);
    expect(isSessionStoreUnavailable(new SessionStoreTimeout('get', 4000, 'circuit-open'))).toBe(true);
  });

  it('is true for an error from a SECOND copy of core/errors', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    // Precondition: without this, the test passes vacuously if the import returns the same module.
    expect(foreign.SessionStoreUnavailable).not.toBe(SessionStoreUnavailable);
    const err = new foreign.SessionStoreTimeout('get', 4000, 'deadline');
    expect(err instanceof SessionStoreUnavailable).toBe(false);
    expect(isSessionStoreUnavailable(err)).toBe(true);
  });

  it('is false for other errors and for non-errors', () => {
    expect(isSessionStoreUnavailable(new CallbackRejected('txn_missing'))).toBe(false);
    expect(isSessionStoreUnavailable(new Error('session_store_unavailable'))).toBe(false);
    expect(isSessionStoreUnavailable({ code: 'session_store_unavailable' })).toBe(false);
    expect(isSessionStoreUnavailable(undefined)).toBe(false);
  });
});

// SMA-657. The same defect as the block above, on the OTHER class that crosses the two copies:
// `refresh` delegates to the shared `runtime.oidc`, so adapters/oidc.ts builds a RefreshRejected
// with the class of whichever copy built the runtime, and core/single-flight.ts classifies it in
// whichever copy serves the request.
describe('isRefreshRejected (SMA-657)', () => {
  it('is true for a RefreshRejected', () => {
    expect(isRefreshRejected(new RefreshRejected('invalid_grant'))).toBe(true);
  });

  it('is true for an error from a SECOND copy of core/errors', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    // Precondition: without this, the test passes vacuously if the import returns the same module.
    expect(foreign.RefreshRejected).not.toBe(RefreshRejected);
    const err = new foreign.RefreshRejected('invalid_grant');
    expect(err instanceof RefreshRejected).toBe(false);
    expect(isRefreshRejected(err)).toBe(true);
  });

  // D8. The check is deliberately WIDER than `instanceof`: `code` is the identity, and admitting
  // any Error that carries it is the whole mechanism by which the foreign-copy instance passes.
  // Pinned here so the semantics are stated rather than discovered.
  it('is true for any Error carrying the code, not only a RefreshRejected', () => {
    expect(isRefreshRejected(Object.assign(new Error('x'), { code: 'oidc_refresh_rejected' }))).toBe(true);
  });

  it('is false for other errors and for non-errors', () => {
    expect(isRefreshRejected(new CallbackRejected('txn_missing'))).toBe(false);
    expect(isRefreshRejected(new SessionStoreUnavailable('down'))).toBe(false);
    expect(isRefreshRejected(new SessionStoreTimeout('get', 4000, 'deadline'))).toBe(false);
    // The literal in the MESSAGE, not in `code`.
    expect(isRefreshRejected(new Error('oidc_refresh_rejected'))).toBe(false);
    // A plain object, not an Error.
    expect(isRefreshRejected({ code: 'oidc_refresh_rejected' })).toBe(false);
    expect(isRefreshRejected(undefined)).toBe(false);
  });
});

// SMA-656 D1. The runtime and its `oidc` client are shared through globalThis, so the adapter can
// build an OidcDiscoveryFailed in one module copy and http/routes.ts can classify it in the other.
describe('isOidcDiscoveryFailed (SMA-656 D1, T10)', () => {
  it('is true for an OidcDiscoveryFailed', () => {
    expect(isOidcDiscoveryFailed(new OidcDiscoveryFailed('oidc discovery failed: TypeError', 'network'))).toBe(true);
  });

  it('is true for an error from a SECOND copy of core/errors', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    // Precondition: without this, the test passes vacuously if the import returns the same module.
    expect(foreign.OidcDiscoveryFailed).not.toBe(OidcDiscoveryFailed);
    const err = new foreign.OidcDiscoveryFailed('oidc discovery failed: TypeError', 'network');
    expect(err instanceof OidcDiscoveryFailed).toBe(false);
    expect(isOidcDiscoveryFailed(err)).toBe(true);
  });

  it('is true for any Error carrying the code, not only an OidcDiscoveryFailed', () => {
    expect(isOidcDiscoveryFailed(Object.assign(new Error('x'), { code: 'oidc_discovery_failed' }))).toBe(true);
  });

  it('is false for other errors and for non-errors', () => {
    // A plain object, not an Error.
    expect(isOidcDiscoveryFailed({ code: 'oidc_discovery_failed' })).toBe(false);
    // The literal in the MESSAGE, not in `code`.
    expect(isOidcDiscoveryFailed(new Error('oidc_discovery_failed'))).toBe(false);
    // What the adapter threw before SMA-656.
    expect(isOidcDiscoveryFailed(new Error('oidc discovery failed: TypeError'))).toBe(false);
    expect(isOidcDiscoveryFailed(new SessionStoreUnavailable('down'))).toBe(false);
    expect(isOidcDiscoveryFailed(new RefreshRejected('invalid_grant'))).toBe(false);
    expect(isOidcDiscoveryFailed(undefined)).toBe(false);
  });

  it('is neither a store failure nor a refresh rejection', () => {
    const err = new OidcDiscoveryFailed('oidc discovery failed: TypeError', 'timeout');
    expect(isSessionStoreUnavailable(err)).toBe(false);
    expect(isRefreshRejected(err)).toBe(false);
  });
});

describe('OidcDiscoveryFailed (SMA-656 D3)', () => {
  it('has a fixed name and code, keeps the message, and has no cause', () => {
    const err = new OidcDiscoveryFailed('oidc discovery failed: ClientError', 'issuer_mismatch');
    expect(err).toBeInstanceOf(AuthError);
    expect(err.name).toBe('OidcDiscoveryFailed');
    expect(err.code).toBe('oidc_discovery_failed');
    expect(err.message).toBe('oidc discovery failed: ClientError');
    expect(err.reason).toBe('issuer_mismatch');
    expect(err.cause).toBeUndefined();
  });
});

describe('oidcDiscoveryReason (SMA-656 D8, T10)', () => {
  it('the closed list is exactly the D8 list, in order', () => {
    expect(OIDC_DISCOVERY_FAILURE_REASONS).toEqual(['timeout', 'network', 'dns', 'tls', 'http_server_error', 'http_client_error', 'invalid_metadata', 'issuer_mismatch', 'other']);
  });

  it.each(OIDC_DISCOVERY_FAILURE_REASONS)('returns %s unchanged', (reason) => {
    expect(oidcDiscoveryReason(new OidcDiscoveryFailed('oidc discovery failed: TypeError', reason))).toBe(reason);
  });

  it.each([
    ['a missing reason', Object.assign(new Error('x'), { code: 'oidc_discovery_failed' })],
    ['a URL', Object.assign(new Error('x'), { code: 'oidc_discovery_failed', reason: 'https://idp.invalid/x' })],
    ['a near miss', Object.assign(new Error('x'), { code: 'oidc_discovery_failed', reason: 'TIMEOUT' })],
    ['a number', Object.assign(new Error('x'), { code: 'oidc_discovery_failed', reason: 42 })],
    ['undefined', undefined],
    ['null', null],
    ['a bare string', 'timeout'],
  ] as const)('returns other for %s', (_label, err) => {
    expect(oidcDiscoveryReason(err)).toBe('other');
  });
});
