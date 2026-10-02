// SPDX-License-Identifier: Apache-2.0
//
// src/prn-tenancy.ts must DELEGATE the PRN grammar to the kernel (SMA-634 spec § 6.3), and it must
// read a PRN with ONE kernel call (SMA-673 A4). The corpus replay in prn-tenancy.test.ts cannot
// prove either: a hand-written reader, or a six-call reader, passes the same rows. This file mocks
// the kernel and asserts that its one answer decides the result.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { prnBuild, prnErrorKind, prnOrg, prnParse, prnRegion, prnResourceId, prnResourceType, prnService, type PrnParseResult } from '@paigasus/kernel';
import { ROOT_PRN, organizationPrn, parsePrincipalPrn, parseTenancyPrn, principalPrn, projectPrn, teamPrn } from '../../src/prn-tenancy';

const { appEvent } = vi.hoisted(() => ({ appEvent: vi.fn() }));

vi.mock('@paigasus/kernel', () => ({
  prnParse: vi.fn(),
  // The six single-field accessors stay in the mock ONLY so the A4 test can assert that they are
  // never called. If prn-tenancy.ts calls one, it gets `undefined`, and the field tests red too.
  prnErrorKind: vi.fn(),
  prnService: vi.fn(),
  prnRegion: vi.fn(),
  prnResourceType: vi.fn(),
  prnResourceId: vi.fn(),
  prnOrg: vi.fn(),
  prnBuild: vi.fn(),
}));
vi.mock('../../src/logger', () => ({ logger: { appEvent } }));

const ORG = '0190a100-0000-7000-8000-0000000000aa';
const TEAM = '0190a1b2-0000-7000-8000-000000000001';
// A PRN that is valid and of a tenancy shape, so only the kernel's verdict can reject it.
const TEAM_PRN = `prn:pgs:iam::${ORG}:team/${TEAM}`;
const PRINCIPAL_PRN = `prn:pgs:iam:::principal/${TEAM}`;
const OLD_ACCESSORS = [prnErrorKind, prnService, prnRegion, prnResourceType, prnResourceId, prnOrg];

type Ok = Extract<PrnParseResult, { ok: true }>;
function ok(fields: Partial<Omit<Ok, 'ok'>> = {}): PrnParseResult {
  return { ok: true, service: 'iam', region: '', org: ORG, resourceType: 'team', resourceId: TEAM, ...fields };
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(prnParse).mockReturnValue(ok());
  vi.mocked(prnBuild).mockReturnValue('built-by-the-kernel');
});

describe('parseTenancyPrn delegates the grammar to the kernel', () => {
  it('returns null when the kernel rejects a PRN that LOOKS valid — a local grammar would accept it', () => {
    vi.mocked(prnParse).mockReturnValue({ ok: false, errorKind: 'wrong-field-count' });
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
    expect(vi.mocked(prnParse)).toHaveBeenCalledWith(TEAM_PRN);
  });

  it('takes the fields from the kernel, not from the string', () => {
    const otherOrg = '0190a100-0000-7000-8000-0000000000bb';
    const otherId = '0190a1b2-0000-7000-8000-000000000002';
    vi.mocked(prnParse).mockReturnValue(ok({ org: otherOrg, resourceId: otherId }));
    // The argument still spells ORG and TEAM. A reader that split the string would return those.
    expect(parseTenancyPrn(TEAM_PRN)).toEqual({ kind: 'team', orgId: otherOrg, id: otherId });
    expect(vi.mocked(prnParse)).toHaveBeenCalledWith(TEAM_PRN);
  });

  it('reads an organization from the kernel, taking its orgId from the resource id', () => {
    const orgId = '0190a100-0000-7000-8000-0000000000cc';
    // An organization carries NO org field, so the kernel reports an empty one.
    vi.mocked(prnParse).mockReturnValue(ok({ resourceType: 'organization', resourceId: orgId, org: '' }));
    expect(parseTenancyPrn(`prn:pgs:iam:::organization/${orgId}`)).toEqual({ kind: 'organization', orgId, id: orgId });
  });

  it('returns null for an organization that the kernel reports WITH an org field', () => {
    vi.mocked(prnParse).mockReturnValue(ok({ resourceType: 'organization' }));
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
  });

  it('returns null when the kernel reports another service', () => {
    vi.mocked(prnParse).mockReturnValue(ok({ service: 'gateway' }));
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
  });

  it('returns null when the kernel reports a region', () => {
    vi.mocked(prnParse).mockReturnValue(ok({ region: 'us-east-1' }));
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
  });

  it('returns null when the kernel reports a non-tenancy resource type', () => {
    vi.mocked(prnParse).mockReturnValue(ok({ resourceType: 'user' }));
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
  });
});

describe('one kernel call per parse (SMA-673 A4)', () => {
  function expectOneCall(prn: string): void {
    expect(vi.mocked(prnParse)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(prnParse)).toHaveBeenCalledWith(prn);
    for (const accessor of OLD_ACCESSORS) expect(vi.mocked(accessor)).not.toHaveBeenCalled();
  }

  it('reads a tenancy PRN with exactly one prnParse call', () => {
    expect(parseTenancyPrn(TEAM_PRN)).toEqual({ kind: 'team', orgId: ORG, id: TEAM });
    expectOneCall(TEAM_PRN);
  });

  it('reads a principal PRN with exactly one prnParse call', () => {
    vi.mocked(prnParse).mockReturnValue(ok({ resourceType: 'principal', org: '' }));
    expect(parsePrincipalPrn(PRINCIPAL_PRN)).toEqual({ id: TEAM });
    expectOneCall(PRINCIPAL_PRN);
  });

  it('reads ROOT_PRN with exactly one prnParse call', () => {
    vi.mocked(prnParse).mockReturnValue(ok({ resourceType: 'root', org: '', resourceId: '00000000-0000-0000-0000-000000000000' }));
    expect(parseTenancyPrn(ROOT_PRN)).toBeNull();
    expectOneCall(ROOT_PRN);
  });
});

/**
 * The `MAX_LEN` guard in src/prn-tenancy.ts is a RESOURCE limit, not grammar. The kernel enforces
 * its own 512-byte rule, but only AFTER the string is copied into wasm linear memory, which never
 * shrinks. The kernel is mocked here and accepts everything, so the ONLY thing that can stop an
 * over-long input is the guard. The assertion is on `prnParse`, the one call that remains
 * (SMA-673): an assertion on the old accessors would stay green with the guard deleted.
 */
describe('the parsers bound the input before the kernel call', () => {
  // MAX_LEN is 512 in src/prn-tenancy.ts. Spelled here so the boundary case below is exact.
  const MAX_LEN = 512;
  const overLong = `prn:pgs:iam::${ORG}:team/${'a'.repeat(MAX_LEN)}`;

  it('does not call the kernel for an over-long PRN', () => {
    expect(overLong.length).toBeGreaterThan(MAX_LEN);
    expect(parseTenancyPrn(overLong)).toBeNull();
    expect(parsePrincipalPrn(overLong)).toBeNull();
    expect(vi.mocked(prnParse)).not.toHaveBeenCalled();
  });

  it('does not call the kernel for an empty PRN', () => {
    expect(parseTenancyPrn('')).toBeNull();
    expect(parsePrincipalPrn('')).toBeNull();
    expect(vi.mocked(prnParse)).not.toHaveBeenCalled();
  });

  // The boundary, so the guard cannot be tightened into a `>=` that rejects a legal PRN, and so the
  // over-long case above is not satisfied by a guard that refuses everything.
  it('passes a PRN of exactly MAX_LEN characters to the kernel, once', () => {
    const head = `prn:pgs:iam::${ORG}:team/`;
    const exact = head + 'a'.repeat(MAX_LEN - head.length);
    expect(exact.length).toBe(MAX_LEN);
    expect(parseTenancyPrn(exact)).toEqual({ kind: 'team', orgId: ORG, id: TEAM });
    expect(vi.mocked(prnParse)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(prnParse)).toHaveBeenCalledWith(exact);
  });
});

/**
 * Both parsers return `… | null`, so they must be TOTAL: a server component calls them on a URL
 * segment, and a throw there is a 500 where a 404 belongs, on input an attacker controls. The one
 * kernel call can still throw: a wasm runtime failure, or the adapter's TypeError on a glue defect.
 * The catch turns it into null AND logs it (SMA-673 D5), because the log line is the only runtime
 * signal of a glue defect. It logs the error's NAME only: never the PRN, never the message.
 */
describe('the parsers are total and log a redacted event when the kernel call fails', () => {
  const MESSAGE_WITH_PRN = `trap while reading ${TEAM_PRN}`;
  const THROWN: ReadonlyArray<readonly [string, unknown, string]> = [
    ['a wasm runtime failure', new Error(MESSAGE_WITH_PRN), 'Error'],
    ["the adapter's TypeError", new TypeError(MESSAGE_WITH_PRN), 'TypeError'],
    ['a non-Error value', MESSAGE_WITH_PRN, 'unknown'],
  ];
  const PARSERS: ReadonlyArray<readonly [string, (prn: string) => unknown]> = [
    ['parseTenancyPrn', parseTenancyPrn],
    ['parsePrincipalPrn', parsePrincipalPrn],
  ];

  for (const [parserName, parse] of PARSERS) {
    it.each(THROWN)(`${parserName} returns null for %s and logs only the error name`, (_label, thrown, name) => {
      vi.mocked(prnParse).mockImplementation(() => {
        throw thrown;
      });
      expect(parse(TEAM_PRN)).toBeNull();
      expect(appEvent).toHaveBeenCalledTimes(1);
      expect(appEvent).toHaveBeenCalledWith('prn.kernel_call_failed', { error: name });
      const logged = JSON.stringify(appEvent.mock.calls);
      expect(logged).not.toContain(TEAM_PRN);
      expect(logged).not.toContain('trap while reading');
    });
  }

  it('does not log when the kernel call succeeds or rejects the PRN', () => {
    parseTenancyPrn(TEAM_PRN);
    vi.mocked(prnParse).mockReturnValue({ ok: false, errorKind: 'bad-org' });
    parseTenancyPrn(TEAM_PRN);
    expect(appEvent).not.toHaveBeenCalled();
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

  it('principalPrn sends an EMPTY org field', () => {
    expect(principalPrn(TEAM)).toBe('built-by-the-kernel');
    expect(vi.mocked(prnBuild)).toHaveBeenCalledWith('iam', '', '', 'principal', TEAM);
  });

  it('rejects an id that is not a UUID before it reaches the kernel', () => {
    expect(() => organizationPrn('x')).toThrow(TypeError);
    expect(vi.mocked(prnBuild)).not.toHaveBeenCalled();
  });

  it('rejects a principal id that is not a UUID before it reaches the kernel', () => {
    expect(() => principalPrn('x')).toThrow(TypeError);
    expect(vi.mocked(prnBuild)).not.toHaveBeenCalled();
  });
});
