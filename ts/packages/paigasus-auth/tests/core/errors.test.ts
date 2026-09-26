// SPDX-License-Identifier: Apache-2.0
//
// SMA-653 D2. Next 16 loads a route handler and a page with SEPARATE copies of this package, and
// the runtime (with its store) is shared between them through globalThis (src/runtime.ts). So a
// store error can be an instance of the OTHER copy's class, and `instanceof` is then false. These
// tests build that second copy with vi.resetModules() and a dynamic import.
import { describe, expect, it, vi } from 'vitest';
import {
  CallbackRejected,
  RefreshFailed,
  RefreshRejected,
  SessionStoreTimeout,
  SessionStoreUnavailable,
  TOKEN_ERROR_CODES,
  isRefreshRejected,
  isSessionStoreUnavailable,
  refreshFailureCode,
  toTokenErrorCode,
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

// SMA-692 D10. The refresh log line carries the OAuth code of a transient failure. The IdP writes
// that code, so only a value of the RFC 6749 § 5.2 list may reach the log. Any other value is
// 'other'.
describe('toTokenErrorCode (SMA-692 D10)', () => {
  it('keeps each code of the closed list', () => {
    for (const code of TOKEN_ERROR_CODES) {
      expect(toTokenErrorCode(code)).toBe(code);
    }
  });

  it('gives other for any value outside the list', () => {
    expect(toTokenErrorCode('server_error')).toBe('other');
    expect(toTokenErrorCode('INVALID_SCOPE')).toBe('other');
    expect(toTokenErrorCode('https://idp.example.com/token?code=abc')).toBe('other');
    expect(toTokenErrorCode(undefined)).toBe('other');
    expect(toTokenErrorCode(42)).toBe('other');
  });
});

describe('refreshFailureCode (SMA-692 D10)', () => {
  it('reads the code of a RefreshFailed', () => {
    expect(refreshFailureCode(new RefreshFailed('invalid_scope', 'ResponseBodyError'))).toBe('invalid_scope');
  });

  it('reads the code of a RefreshFailed from a SECOND copy of core/errors', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    // Precondition: without this, the test passes vacuously if the import returns the same module.
    expect(foreign.RefreshFailed).not.toBe(RefreshFailed);
    expect(refreshFailureCode(new foreign.RefreshFailed('invalid_client', 'ResponseBodyError'))).toBe('invalid_client');
  });

  it('gives other for an Error that only claims the code and holds any string', () => {
    const forged = Object.assign(new Error('x'), { code: 'oidc_refresh_failed', oauthError: 'https://idp.example.com/secret' });
    expect(refreshFailureCode(forged)).toBe('other');
  });

  it('is undefined for every other error and for a non-error', () => {
    expect(refreshFailureCode(new RefreshRejected('invalid_grant'))).toBeUndefined();
    expect(refreshFailureCode(new Error('oidc refresh_token_grant failed: TypeError'))).toBeUndefined();
    expect(refreshFailureCode({ code: 'oidc_refresh_failed', oauthError: 'invalid_scope' })).toBeUndefined();
    expect(refreshFailureCode(undefined)).toBeUndefined();
  });

  it('keeps the message free of the code and of any URL', () => {
    const err = new RefreshFailed('invalid_scope', 'ResponseBodyError');
    expect(err.message).toBe('oidc refresh_token_grant failed: ResponseBodyError');
    expect(err.code).toBe('oidc_refresh_failed');
  });
});
