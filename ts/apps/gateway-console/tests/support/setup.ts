// SPDX-License-Identifier: Apache-2.0
//
// Runs before every test file (vitest.config.ts `setupFiles`). It resets the Next headers/cookies
// double, so no test sees another test's request headers or cookies.
import { beforeEach } from 'vitest';
import { resetNextHeaders } from './next-headers';

beforeEach(() => {
  resetNextHeaders();
});
