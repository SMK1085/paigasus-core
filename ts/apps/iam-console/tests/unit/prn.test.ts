// SPDX-License-Identifier: Apache-2.0
//
// lib/prn.ts against the kernel (SMA-511 spec § 4.7, decision D6). The corpus rows are the SAME
// vectors every kernel binding replays (rs/crates/libs/paigasus-kernel-parity/vectors/), so a reader
// that drifts from the Rust grammar fails here. This suite tests the INTERFACE, so it is the same for
// both implementations of lib/prn.ts (the kernel wasm binding, or the fallback reader).
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { ROOT_PRN, isUuid, organizationPrn, parseTenancyPrn, projectPrn, teamPrn, type TenancyRef } from '../../lib/prn';

// tests/unit -> tests -> iam-console -> apps -> ts -> repo root: five `../`.
const REPO_ROOT = new URL('../../../../../', import.meta.url);

function read(rel: string): string {
  return readFileSync(fileURLToPath(new URL(rel, REPO_ROOT)), 'utf8');
}

function vectors<T>(name: string): T[] {
  return JSON.parse(read(`rs/crates/libs/paigasus-kernel-parity/vectors/${name}.json`)) as T[];
}

type FieldsCase = { prn: string; service: string; region: string; org: string; resource_type: string; resource_id: string };
type CanonicalCase = { input: string; error_kind: string; canonical: string | null };

const fieldsCases = vectors<FieldsCase>('prn_fields');
const canonicalCases = vectors<CanonicalCase>('prn_canonical');
const TENANCY = new Set(['organization', 'team', 'project']);

const ORG = '0190a100-0000-7000-8000-0000000000aa';
const TEAM = '0190a1b2-0000-7000-8000-000000000001';

/** IAM's tenancy rule (paigasus-iam-core tenancy.rs `check`): organization has no org field; team and project have one. */
function expectedRef(c: FieldsCase): TenancyRef | null {
  if (c.service !== 'iam' || !TENANCY.has(c.resource_type)) return null;
  if (c.resource_type === 'organization') return c.org === '' ? { kind: 'organization', orgId: c.resource_id, id: c.resource_id } : null;
  return c.org === '' ? null : { kind: c.resource_type as 'team' | 'project', orgId: c.org, id: c.resource_id };
}

function build(ref: TenancyRef): string {
  switch (ref.kind) {
    case 'organization':
      return organizationPrn(ref.id);
    case 'team':
      return teamPrn(ref.orgId, ref.id);
    case 'project':
      return projectPrn(ref.orgId, ref.id);
  }
}

describe('lib/prn.ts against the kernel parity corpus', () => {
  // Without this, an emptied or reshaped corpus would make every case below pass vacuously.
  it('holds a vector of each tenancy kind, a non-tenancy vector and an invalid vector', () => {
    const kinds = new Set(
      fieldsCases
        .map(expectedRef)
        .filter((ref) => ref !== null)
        .map((ref) => ref.kind),
    );
    expect([...kinds].sort()).toEqual(['organization', 'project', 'team']);
    expect(fieldsCases.some((c) => expectedRef(c) === null)).toBe(true);
    expect(canonicalCases.some((c) => c.error_kind !== '')).toBe(true);
  });

  it.each(fieldsCases)('prn_fields: $prn', (c) => {
    const expected = expectedRef(c);
    expect(parseTenancyPrn(c.prn)).toEqual(expected);
    if (expected !== null) expect(build(expected)).toBe(c.prn);
  });

  it.each(canonicalCases.map((c, index) => [index, c] as const))('prn_canonical vector %i', (_index, c) => {
    if (c.error_kind !== '') {
      expect(parseTenancyPrn(c.input)).toBeNull();
      return;
    }
    const canonical = c.canonical ?? '';
    const ref = parseTenancyPrn(canonical);
    expect(parseTenancyPrn(c.input)).toEqual(ref);
    if (ref !== null) expect(build(ref)).toBe(canonical);
  });
});

describe('ROOT_PRN', () => {
  it('is the canonical form of IAM root_prn()', () => {
    expect(ROOT_PRN).toBe('prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000');
  });

  // A change to root_prn()'s arguments must re-derive ROOT_PRN. The app's test task lists model.rs
  // among its inputs, so an edit there re-runs this assertion.
  it('still matches the Rust builder call it is derived from', () => {
    expect(read('rs/crates/libs/paigasus-iam-core/src/authz/model.rs')).toMatch(/pub fn root_prn\(\) -> Prn \{\s*Prn::build\("iam", "", None, "root", Uuid::nil\(\)\)/);
  });

  it('is not a tenancy PRN', () => {
    expect(parseTenancyPrn(ROOT_PRN)).toBeNull();
  });
});

describe('parseTenancyPrn', () => {
  it('lower-cases the ids of an upper-case PRN, as the kernel canonicalises', () => {
    expect(parseTenancyPrn(`prn:pgs:iam::${ORG.toUpperCase()}:team/${TEAM.toUpperCase()}`)).toEqual({ kind: 'team', orgId: ORG, id: TEAM });
  });

  it.each([
    ['a team without an org field', `prn:pgs:iam:::team/${TEAM}`],
    ['an organization WITH an org field', `prn:pgs:iam::${ORG}:organization/${TEAM}`],
    ['another service', `prn:pgs:gateway::${ORG}:team/${TEAM}`],
    ['a non-tenancy IAM type', `prn:pgs:iam:::user/${TEAM}`],
    ['a malformed org UUID', `prn:pgs:iam::not-a-uuid:team/${TEAM}`],
    ['an upper-case region', `prn:pgs:iam:US-EAST:${ORG}:team/${TEAM}`],
    ['two slashes in the resource path', `prn:pgs:iam::${ORG}:team/${TEAM}/x`],
    ['the empty string', ''],
    // Valid in every field except its length: 12 + 480 + 1 + 36 + 1 + 5 + 36 = 571 characters. The
    // kernel sets no region length limit other than MAX_LEN (512), so ONLY the length rule rejects it.
    ['a valid team PRN of 571 characters (a 480-character region)', `prn:pgs:iam:${'a'.repeat(480)}:${ORG}:team/${TEAM}`],
  ])('returns null for %s', (_label, prn) => {
    expect(parseTenancyPrn(prn)).toBeNull();
  });
});

describe('isUuid', () => {
  it.each(['0190a1b2-0000-7000-8000-000000000001', '0190A1B2-0000-7000-8000-00000000ABCD', '00000000-0000-0000-0000-000000000000'])('accepts %s', (value) => {
    expect(isUuid(value)).toBe(true);
  });

  it.each(['', 'not-a-uuid', '0190a1b2000070008000000000000001', '{0190a1b2-0000-7000-8000-000000000001}', '0190a1b2-0000-7000-8000-00000000000g', ' 0190a1b2-0000-7000-8000-000000000001'])(
    'rejects %j',
    (value) => {
      expect(isUuid(value)).toBe(false);
    },
  );
});

describe('the builders', () => {
  it('build canonical, lower-case PRNs', () => {
    expect(organizationPrn(ORG.toUpperCase())).toBe(`prn:pgs:iam:::organization/${ORG}`);
    expect(teamPrn(ORG, TEAM)).toBe(`prn:pgs:iam::${ORG}:team/${TEAM}`);
    expect(projectPrn(ORG, TEAM)).toBe(`prn:pgs:iam::${ORG}:project/${TEAM}`);
  });

  it('throw a TypeError for an id that is not a UUID, so a bad URL segment cannot become a PRN', () => {
    expect(() => organizationPrn('x')).toThrow(TypeError);
    expect(() => teamPrn(ORG, '../x')).toThrow(TypeError);
    expect(() => projectPrn('x', TEAM)).toThrow(TypeError);
  });
});
