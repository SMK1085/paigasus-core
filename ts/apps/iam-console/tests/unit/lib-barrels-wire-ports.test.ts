// SPDX-License-Identifier: Apache-2.0
//
// TEMPORARY (SMA-512 PR 2, task 4 → task 5). This guards a PROPERTY, not the mechanism that
// currently provides it: importing any of the five barrels in
// lib/{iam,principal,authorize,scopes,discovery}.ts must, by itself, wire
// @paigasus/console-core's ports before an accessor is ever called.
//
// THE REGRESSION THIS PINS (task 4 fix round 1). Before this task, lib/iam.ts did
// `import { authRuntime } from './auth'`, so importing it — which nearly every page, load and
// action file does — transitively ran lib/auth.ts's module body. Task 4 replaced lib/iam.ts (and
// the other four) with a barrel that only re-exports from `@paigasus/console-core`, which has no
// edge back to the app's lib/auth.ts. That silently dropped the guarantee: on a fresh process, the
// first request to any accessor before app/auth/[...auth]/route.ts (the one remaining production
// importer of lib/auth.ts) had run threw "createConsoleRuntime() was never called" instead of
// requireSession() redirecting to login — a 500 where a redirect belongs.
//
// THE FIX each barrel now carries `import './auth'` for its side effect (lib/iam.ts's own comment
// has the full story). This test proves that side effect actually runs, not that some OTHER test
// happened to import lib/auth.ts first: vi.resetModules() gives each case a completely fresh
// @paigasus/console-core module instance, so no earlier test's setConsolePorts() call can hide a
// regression here.
//
// It does NOT assert the accessor call succeeds — with no real env configured, the real
// authRuntime()/getRuntimeConfig() a wired port now reaches will throw ITS OWN error (invalid
// config) or, for principal/authorize/scopes, a redirect. Both are fine: this test only checks
// that the ONE specific "ports were never set" failure is not among them. Task 5's
// createConsoleRuntime() call replaces the mechanism; whoever rewrites this test there should keep
// asserting the same property (an accessor call reaches real port-backed logic on import alone),
// not this barrel-specific wiring.
import { describe, expect, it, vi } from 'vitest';

const PORTS_UNSET_MESSAGE = 'createConsoleRuntime() was never called';

/** True only when `run()` fails with the specific "ports were never set" error — sync or async. */
async function failedBecausePortsUnset(run: () => unknown): Promise<boolean> {
  try {
    await run();
    return false;
  } catch (err) {
    return err instanceof Error && err.message.includes(PORTS_UNSET_MESSAGE);
  }
}

describe('the five lib/ barrels wire @paigasus/console-core’s ports on import alone', () => {
  it.each([
    ['lib/iam.ts', async () => (await import('../../lib/iam')).optionalSession()],
    ['lib/principal.ts', async () => (await import('../../lib/principal')).currentPrincipal()],
    ['lib/authorize.ts', async () => (await import('../../lib/authorize')).mayI()],
    ['lib/scopes.ts', async () => (await import('../../lib/scopes')).myScopes()],
    ['lib/discovery.ts', () => import('../../lib/discovery').then((m) => m.discovery())],
  ] as const)('%s', async (_label, callAccessor) => {
    vi.resetModules();
    expect(await failedBecausePortsUnset(callAccessor)).toBe(false);
  });
});
