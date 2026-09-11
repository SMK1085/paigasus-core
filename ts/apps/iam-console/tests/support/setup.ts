// SPDX-License-Identifier: Apache-2.0
//
// Runs before every test file (vitest.config.ts `setupFiles`). It resets the two Next doubles, so
// no test sees another test's request headers, cookies or revalidations.
import { beforeEach } from 'vitest';
import { resetNextCache } from './next-cache';
import { resetNextHeaders } from './next-headers';

beforeEach(() => {
  resetNextHeaders();
  resetNextCache();
});
