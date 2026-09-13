// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { sameNode } from '../../app/(console)/orgs/node-ref';
import { organizationPrn, projectPrn, teamPrn } from '../../lib/prn-tenancy';

const ORG = '0190a100-0000-7000-8000-00000000000a';
const OTHER = '0190a100-0000-7000-8000-00000000000b';
const TEAM = '0190a1b2-0000-7000-8000-0000000000a1';

describe('sameNode', () => {
  it('matches the kind and the id, in any UUID case', () => {
    expect(sameNode(organizationPrn(ORG), 'organization', ORG)).toBe(true);
    expect(sameNode(organizationPrn(ORG), 'organization', ORG.toUpperCase())).toBe(true);
    expect(sameNode(teamPrn(ORG, TEAM), 'team', TEAM)).toBe(true);
  });

  it('refuses another id, another kind, and a PRN that does not parse', () => {
    expect(sameNode(organizationPrn(OTHER), 'organization', ORG)).toBe(false);
    expect(sameNode(teamPrn(ORG, TEAM), 'organization', TEAM)).toBe(false);
    expect(sameNode(projectPrn(ORG, TEAM), 'team', TEAM)).toBe(false);
    expect(sameNode('', 'team', TEAM)).toBe(false);
    expect(sameNode('not a prn', 'organization', ORG)).toBe(false);
  });
});
