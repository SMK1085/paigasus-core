// SPDX-License-Identifier: Apache-2.0
import { vi } from 'vitest';

/**
 * React logs every error thrown during render to console.error before it re-throws. Silence that
 * log in a test that EXPECTS a render to throw. Restore it with `vi.restoreAllMocks()` in afterEach.
 */
export function silenceReactErrorLog(): void {
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
}
