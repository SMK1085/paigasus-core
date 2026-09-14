// SPDX-License-Identifier: Apache-2.0
//
// The property `lib-barrels-wire-ports.test.ts` used to guard, carried over to the new shape
// (SMA-512 PR 2, task 5, controller ruling B).
//
// THE OLD MECHANISM. Task 4's five barrels (lib/{iam,principal,authorize,scopes,discovery}.ts)
// re-exported from @paigasus/console-core, which read a package-wide "ports" singleton an app had
// to set once, separately, before any accessor ran. A refactor silently dropped that separate step
// once already: the first request in a fresh process to any accessor before lib/auth.ts's
// module-scope setConsolePorts() call had run threw "createConsoleRuntime() was never called" —
// the getter's own error message named task 5's not-yet-written factory, not the setConsolePorts()
// call that was actually missing — instead of requireSession() redirecting to login, a 500 where a
// redirect belongs.
//
// THE NEW SHAPE removes the separate step structurally: lib/console.ts's ONE
// createConsoleRuntime() call composes the app's real authRuntime and getRuntimeConfig() at module
// scope, so an ordinary import of lib/console.ts is enough to get working accessors. There is no
// "ports" singleton left to forget to wire.
//
// WHAT THIS TEST STILL GUARDS. A future edit could turn that module-scope call into something
// lazy, partial, or built from the wrong pieces, so that importing lib/console.ts alone no longer
// yields a complete, callable runtime. This test proves, on a fresh module (vi.resetModules()),
// that every accessor exists as a callable function on import alone, and that calling one fails —
// with no environment configured for this test file, getRuntimeConfig() fails closed, which is
// fine — with the app's own domain error, never a TypeError from an undefined composition (a
// missing field, or something that is not a function).
import { describe, expect, it, vi } from 'vitest';

const ACCESSORS = ['currentSession', 'optionalSession', 'sessionToken', 'iamClients', 'iamClientsForAction', 'iamClientsForToken', 'currentPrincipal', 'mayI', 'myScopes', 'discovery'] as const;

describe('lib/console.ts composes a complete runtime on import alone', () => {
  it('exports every ConsoleRuntime accessor as a callable function', async () => {
    vi.resetModules();
    const mod: Record<string, unknown> = await import('../../lib/console');
    for (const name of ACCESSORS) {
      expect(typeof mod[name]).toBe('function');
    }
  });

  it('a failing accessor fails with the app’s own domain error, never a wiring TypeError', async () => {
    vi.resetModules();
    const { optionalSession } = await import('../../lib/console');

    const error = await optionalSession().then(
      () => null,
      (err: unknown) => err,
    );

    // No environment is stubbed for this test file, so getRuntimeConfig() fails closed with its
    // own plain Error. A TypeError here (e.g. "X is not a function") would mean the composition
    // itself — not the environment — is broken.
    expect(error).not.toBeNull();
    expect(error).not.toBeInstanceOf(TypeError);
  });
});
