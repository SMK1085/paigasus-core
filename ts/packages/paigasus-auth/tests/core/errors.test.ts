// SPDX-License-Identifier: Apache-2.0
//
// SMA-653 D2. Next 16 loads a route handler and a page with SEPARATE copies of this package, and
// the runtime (with its store) is shared between them through globalThis (src/runtime.ts). So a
// store error can be an instance of the OTHER copy's class, and `instanceof` is then false. These
// tests build that second copy with vi.resetModules() and a dynamic import.
import { describe, expect, it, vi } from 'vitest';
import { CallbackRejected, SessionStoreTimeout, SessionStoreUnavailable, isSessionStoreUnavailable } from '../../src/core/errors.js';

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
