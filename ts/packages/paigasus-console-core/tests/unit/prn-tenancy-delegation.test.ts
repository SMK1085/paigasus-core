// SPDX-License-Identifier: Apache-2.0
//
// src/prn-tenancy.ts must DELEGATE the PRN grammar to the kernel (SMA-634 spec § 6.3). The corpus
// replay in prn-tenancy.test.ts cannot prove that: the hand-written reader it replaced passed the
// same rows, and so would a text check, because that reader's only regex was the UUID one. This
// file mocks the kernel and asserts that its answers decide the result.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { prnBuild, prnErrorKind, prnOrg, prnRegion, prnResourceId, prnResourceType, prnService } from '@paigasus/kernel';
import { organizationPrn, parseTenancyPrn, projectPrn, teamPrn } from '../../src/prn-tenancy';

vi.mock('@paigasus/kernel', () => ({
  prnErrorKind: vi.fn(),
  prnService: vi.fn(),
  prnRegion: vi.fn(),
  prnResourceType: vi.fn(),
  prnResourceId: vi.fn(),
  prnOrg: vi.fn(),
  prnBuild: vi.fn(),
}));

const ORG = '0190a100-0000-7000-8000-0000000000aa';
const TEAM = '0190a1b2-0000-7000-8000-000000000001';
// A PRN that is valid and of a tenancy shape, so only the kernel's verdict can reject it.
const TEAM_PRN = `prn:pgs:iam::${ORG}:team/${TEAM}`;

beforeEach(() => {
  vi.mocked(prnErrorKind).mockReturnValue('');
  vi.mocked(prnService).mockReturnValue('iam');
  vi.mocked(prnRegion).mockReturnValue('');
  vi.mocked(prnResourceType).mockReturnValue('team');
  vi.mocked(prnResourceId).mockReturnValue(TEAM);
  vi.mocked(prnOrg).mockReturnValue(ORG);
  vi.mocked(prnBuild).mockReturnValue('built-by-the-kernel');
});

describe('parseTenancyPrn delegates the grammar to the kernel', () => {
  it('returns null when the kernel rejects a PRN that LOOKS valid — a local grammar would accept it', () => {
    vi.mocked(prnErrorKind).mockReturnValue('wrong-field-count');
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
    expect(vi.mocked(prnErrorKind)).toHaveBeenCalledWith(TEAM_PRN);
  });

  it('takes the fields from the kernel, not from the string', () => {
    const otherOrg = '0190a100-0000-7000-8000-0000000000bb';
    const otherId = '0190a1b2-0000-7000-8000-000000000002';
    vi.mocked(prnOrg).mockReturnValue(otherOrg);
    vi.mocked(prnResourceId).mockReturnValue(otherId);
    // The argument still spells ORG and TEAM. A reader that split the string would return those.
    expect(parseTenancyPrn(TEAM_PRN)).toEqual({ kind: 'team', orgId: otherOrg, id: otherId });
    // Each field accessor must be asked about THIS PRN. Without these, an adapter that passed a
    // constant or the wrong string to an accessor would still satisfy the assertion above.
    for (const accessor of [prnService, prnRegion, prnResourceType, prnResourceId, prnOrg]) {
      expect(vi.mocked(accessor)).toHaveBeenCalledWith(TEAM_PRN);
    }
  });

  it('reads an organization from the kernel, taking its orgId from the resource id', () => {
    const orgId = '0190a100-0000-7000-8000-0000000000cc';
    vi.mocked(prnResourceType).mockReturnValue('organization');
    vi.mocked(prnResourceId).mockReturnValue(orgId);
    // An organization carries NO org field, so the kernel reports an empty one.
    vi.mocked(prnOrg).mockReturnValue('');
    expect(parseTenancyPrn(`prn:pgs:iam:::organization/${orgId}`)).toEqual({ kind: 'organization', orgId, id: orgId });
  });

  it('returns null for an organization that the kernel reports WITH an org field', () => {
    vi.mocked(prnResourceType).mockReturnValue('organization');
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
  });

  it('returns null when the kernel reports another service', () => {
    vi.mocked(prnService).mockReturnValue('gateway');
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
  });

  it('returns null when the kernel reports a region', () => {
    vi.mocked(prnRegion).mockReturnValue('us-east-1');
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
  });

  it('returns null when the kernel reports a non-tenancy resource type', () => {
    vi.mocked(prnResourceType).mockReturnValue('user');
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
  });
});

/**
 * `parseTenancyPrn` returns `TenancyRef | null`, so it must be TOTAL: no input may make it throw.
 * The adapter reads six kernel functions, and every one of them can throw. An empty `prnErrorKind`
 * happens to imply the other five succeed today, because all seven call the same `Prn::parse` — but
 * nothing pins that. A wasm runtime failure, or an accessor that one day validates more than
 * `parse` does, would break it. The caller is a server component reading a URL segment, so a throw
 * there is a 500 where a 404 belongs, on input an attacker controls.
 */
describe('parseTenancyPrn is total when a kernel call fails', () => {
  const ACCESSORS: ReadonlyArray<readonly [string, (s: string) => string]> = [
    ['prnErrorKind', prnErrorKind],
    ['prnService', prnService],
    ['prnRegion', prnRegion],
    ['prnResourceType', prnResourceType],
    ['prnResourceId', prnResourceId],
    ['prnOrg', prnOrg],
  ];

  it.each(ACCESSORS)('returns null when %s throws, although the error kind is empty', (_name, accessor) => {
    vi.mocked(accessor).mockImplementation(() => {
      throw new Error('wasm trap');
    });
    expect(() => parseTenancyPrn(TEAM_PRN)).not.toThrow();
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
  });

  it('does not swallow a failure of the builders, which have no null contract', () => {
    vi.mocked(prnBuild).mockImplementation(() => {
      throw new Error('wasm trap');
    });
    expect(() => teamPrn(ORG, TEAM)).toThrow('wasm trap');
  });
});

describe('the builders call prnBuild with the IAM tenancy arguments', () => {
  it('organizationPrn sends an EMPTY org field', () => {
    expect(organizationPrn(ORG)).toBe('built-by-the-kernel');
    expect(vi.mocked(prnBuild)).toHaveBeenCalledWith('iam', '', '', 'organization', ORG);
  });

  it('teamPrn sends the org field', () => {
    expect(teamPrn(ORG, TEAM)).toBe('built-by-the-kernel');
    expect(vi.mocked(prnBuild)).toHaveBeenCalledWith('iam', '', ORG, 'team', TEAM);
  });

  it('projectPrn sends the org field', () => {
    expect(projectPrn(ORG, TEAM)).toBe('built-by-the-kernel');
    expect(vi.mocked(prnBuild)).toHaveBeenCalledWith('iam', '', ORG, 'project', TEAM);
  });

  it('rejects an id that is not a UUID before it reaches the kernel', () => {
    expect(() => organizationPrn('x')).toThrow(TypeError);
    expect(vi.mocked(prnBuild)).not.toHaveBeenCalled();
  });
});
