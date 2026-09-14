// SPDX-License-Identifier: Apache-2.0
//
// switcherOrgs (spec § 5.4) lists the organizations from myScopes(). The (console) layout in the
// app passes switcherOrgs(await myScopes()) to OrgSwitcherShell, which builds the hrefs itself. One
// entry per ORGANIZATION scope, in myScopes() order. A row with no name (a denied row, or one whose
// GetOrganization failed) shows its UUID. An IAM error gives an empty switcher, never a failed
// layout. The (console) layout's own test stays in the app
// (ts/apps/iam-console/tests/unit/switcher-orgs.test.ts) — this file was split off it (SMA-512 PR
// 2, task 4) when scopes.ts moved here.
import { describe, expect, it } from 'vitest';
import { sessionExpired } from '../../src/errors';
import { switcherOrgs, type MyScopes, type ScopeEntry } from '../../src/scopes';

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

  // Any PaigasusError this package itself can produce works here — the case only needs an `ok:
  // false` result. sessionExpired() stands in for the app's invalidFormInput(), which lives in
  // lib/form.ts and stays in the app.
  it('gives an empty list when myScopes() failed, so the layout still renders', () => {
    expect(switcherOrgs({ ok: false, error: sessionExpired() })).toEqual([]);
  });
});
