// SPDX-License-Identifier: Apache-2.0
//
// lib/console.ts's ONE createConsoleRuntime() call, read as TEXT (SMA-512). A future edit could
// turn that module-scope call into something lazy, partial, or built from the wrong pieces — in
// particular, it could pass the imported `authRuntime` binding straight through instead of a fresh
// arrow, which would reintroduce the TDZ hazard CLAUDE.md records for this pair of modules
// (SMA-511). No unit or integration test can observe the runtime consequence of either mistake
// directly (createConsoleRuntime's accessors are React cache() wrappers, a pass-through outside a
// server render), so this test asserts the SOURCE shape instead.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const source = readFileSync(new URL('../../lib/console.ts', import.meta.url), 'utf8');

// lib/console.ts's own header comment QUOTES the exact call-site shape this suite pins
// ("authRuntime: () => authRuntime()"), as prose explaining why the call site is written that
// way. MEASURED: a plain regex over `source` matches that prose even when the call site itself
// regresses to the bare imported binding (`authRuntime,`), because the comment string survives
// untouched. The fresh-arrow assertion below therefore runs against comment-stripped source, so it
// binds to the call site and not to documentation about the call site.
const codeOnly = source
  .split('\n')
  .filter((line) => !line.trim().startsWith('//'))
  .join('\n');

describe('lib/console.ts composes the runtime exactly once, correctly', () => {
  it('calls createConsoleRuntime exactly once', () => {
    expect(source.match(/createConsoleRuntime\s*\(/g) ?? []).toHaveLength(1);
  });

  it('calls it at module scope, not inside a function', () => {
    expect(source).toMatch(/^export const \{[^}]*\} = createConsoleRuntime\(/m);
  });

  it('re-exports every accessor the app uses', () => {
    for (const name of ['currentSession', 'optionalSession', 'sessionToken', 'iamClients', 'iamClientsForAction', 'iamClientsForToken', 'currentPrincipal', 'mayI', 'myScopes', 'discovery']) {
      expect(source).toContain(name);
    }
  });

  it('passes the auth runtime as a fresh arrow, not the imported binding', () => {
    expect(codeOnly).toMatch(/authRuntime:\s*\(\)\s*=>\s*authRuntime\(\)/);
  });
});
