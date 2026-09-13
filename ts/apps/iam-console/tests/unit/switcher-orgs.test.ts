// SPDX-License-Identifier: Apache-2.0
//
// The organization switcher (spec § 5.4) lists the organizations from myScopes(). The (console)
// layout passes switcherOrgs(await myScopes()) to OrgSwitcherShell (Task 15), which builds the
// hrefs itself. One entry per ORGANIZATION scope, in myScopes() order. A row with no name (a denied
// row, or one whose GetOrganization failed) shows its UUID. An IAM error gives an empty switcher,
// never a failed layout.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { invalidFormInput } from '../../lib/form';
import { switcherOrgs, type MyScopes, type ScopeEntry } from '../../lib/scopes';

const ORG_A = '0190a100-0000-7000-8000-00000000000a';
const ORG_B = '0190a100-0000-7000-8000-00000000000b';
const ORG_C = '0190a100-0000-7000-8000-00000000000c';
const TEAM = '0190a1b2-0000-7000-8000-0000000000a1';
const PROJECT = '0190a1c3-0000-7000-8000-0000000000c1';

const TEAM_ENTRY: ScopeEntry = { kind: 'team', prn: `prn:pgs:iam::${ORG_A}:team/${TEAM}`, orgId: ORG_A, teamId: TEAM, label: 'Platform', denied: false };

const SCOPES: MyScopes = {
  grantsListed: true,
  hiddenCount: 0,
  entries: [
    { kind: 'organization', prn: `prn:pgs:iam:::organization/${ORG_A}`, orgId: ORG_A, label: 'Acme', denied: false },
    TEAM_ENTRY,
    { kind: 'organization', prn: `prn:pgs:iam:::organization/${ORG_B}`, orgId: ORG_B, label: null, denied: true },
    { kind: 'organization', prn: `prn:pgs:iam:::organization/${ORG_C}`, orgId: ORG_C, label: null, denied: false },
    { kind: 'project', prn: `prn:pgs:iam::${ORG_C}:project/${PROJECT}`, orgId: ORG_C, teamId: null, projectId: PROJECT, label: 'Models', denied: false },
  ],
};

const LAYOUT = fileURLToPath(new URL('../../app/(console)/layout.tsx', import.meta.url));

describe('switcherOrgs', () => {
  it('lists one entry per organization scope, in order, and skips the team and project scopes', () => {
    expect(switcherOrgs({ ok: true, value: SCOPES })).toEqual([
      { orgId: ORG_A, label: 'Acme' },
      { orgId: ORG_B, label: ORG_B },
      { orgId: ORG_C, label: ORG_C },
    ]);
  });

  it('gives an empty list when the principal has no organization scope', () => {
    expect(switcherOrgs({ ok: true, value: { grantsListed: false, hiddenCount: 0, entries: [TEAM_ENTRY] } })).toEqual([]);
  });

  it('gives an empty list when myScopes() failed, so the layout still renders', () => {
    expect(switcherOrgs({ ok: false, error: invalidFormInput() })).toEqual([]);
  });
});

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
