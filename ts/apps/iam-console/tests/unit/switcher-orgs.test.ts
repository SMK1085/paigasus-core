// SPDX-License-Identifier: Apache-2.0
//
// The (console) layout wires switcherOrgs(await myScopes()) — now moved to
// @paigasus/console-core (SMA-512 PR 2, task 4) — into OrgSwitcherShell (Task 15), which builds
// the hrefs itself. switcherOrgs() itself is tested with scopes.ts, in the package.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const LAYOUT = fileURLToPath(new URL('../../app/(console)/layout.tsx', import.meta.url));

describe('the (console) layout', () => {
  it('renders OrgSwitcherShell with switcherOrgs(myScopes()), and no bare AppShell', () => {
    // Strip the line comments first: the layout's comments name myScopes(), and so did Task 15's
    // layout, which had no myScopes() call at all.
    const source = readFileSync(LAYOUT, 'utf8').replace(/^\s*\/\/.*$/gm, '');
    // The call must sit inside the layout's Promise.all([…]), the one that yields `scopes`.
    expect(source).toMatch(/Promise\.all\(\[[^\]]*\bmyScopes\(\)/);
    expect(source).toMatch(/<OrgSwitcherShell\b[^>]*\borgs=\{switcherOrgs\(scopes\)\}/);
    expect(source).not.toMatch(/<AppShell\b/);
  });
});
