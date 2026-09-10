// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const pkg = JSON.parse(readFileSync(fileURLToPath(new URL('../../package.json', import.meta.url)), 'utf8')) as { exports: Record<string, string>; dependencies: Record<string, string> };

describe('package boundary', () => {
  it('exposes exactly three subpaths and NO root export', () => {
    // A root export re-exporting the server surface would let a client component import it and
    // walk around the boundary — the reason @paigasus/auth omits one too.
    expect(Object.keys(pkg.exports).sort()).toEqual(['./react', './server', './types']);
    expect(pkg.exports['.']).toBeUndefined();
  });

  it('does not depend on @paigasus/sdk', () => {
    // Three reasons: the SDK's Presentation union cannot express a DegradedReason; the probe is
    // a bare fetch, so @connectrpc/* is pure cost; and `paigasus/boundaries/app-shell` bans the
    // SDK from app-shell, which a transitive edge through this package would violate.
    expect(Object.keys(pkg.dependencies)).not.toContain('@paigasus/sdk');
  });

  it('guards the server entries and deliberately leaves ./types unguarded', () => {
    const read = (rel: string): string => readFileSync(fileURLToPath(new URL(`../../${rel}`, import.meta.url)), 'utf8');
    expect(read('src/server.ts')).toMatch(/^import 'server-only';/m);
    expect(read('src/react.tsx')).toMatch(/^import 'server-only';/m);
    // ./types is client-safe on purpose: a client component holds a resolved ServiceState.
    expect(read('src/types.ts')).not.toMatch(/server-only/);
  });

  it('never sets the react-server vitest condition', () => {
    // MEASURED in @paigasus/auth: react-server is also the condition `react`'s own exports map
    // switches on, and that build has no createContext, so setting it breaks react-dom/client
    // under @testing-library/react. The `server-only` alias is the fix instead.
    //
    // The assertion checks for the QUOTED condition value, not the bare word: this file's own
    // comment explaining the omission legitimately contains the bare word "react-server" in
    // backticks, and a plain substring check would fail against that prose rather than against
    // an actual condition setting.
    const config = readFileSync(fileURLToPath(new URL('../../vitest.config.ts', import.meta.url)), 'utf8');
    expect(config).not.toContain("'react-server'");
    expect(config).toContain("alias: { 'server-only'");
  });
});
