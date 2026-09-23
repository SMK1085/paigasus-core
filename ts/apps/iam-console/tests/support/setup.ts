// SPDX-License-Identifier: Apache-2.0
//
// Runs before every test file (vitest.config.ts `setupFiles`). It resets the two Next doubles, so
// no test sees another test's request headers, cookies or revalidations.
import { assertInstalledWasmMatchesCommitted } from '@paigasus/console-core/testing';
import { beforeEach } from 'vitest';
import { resetNextCache } from './next-cache';
import { resetNextHeaders } from './next-headers';

// SMA-634 check 4. At module scope on purpose: a stale installed @paigasus/wasm makes every test in
// this file run against another commit's kernel, so the file must fail at once with the repair
// command rather than report a confusing assertion.
assertInstalledWasmMatchesCommitted();

beforeEach(() => {
  resetNextHeaders();
  resetNextCache();
});
