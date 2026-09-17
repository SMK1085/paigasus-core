// SPDX-License-Identifier: Apache-2.0
//
// IAM_ACTIONS against the action catalog of IAM (SMA-630 spec § 5.1). mayI() FAILS OPEN: IAM
// answers a misspelt action name with `invalid-action`, mayI() then answers true, and the control
// always shows. No page test can see that. This test holds every name to the wire names in the
// Rust catalog, which it reads as TEXT from the `as_wire` match arms.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { IAM_ACTIONS } from '../../src/authorize';

// tests/unit -> tests -> paigasus-console-core -> packages -> ts -> repo root: five `../`, the same
// depth as tests/unit/prn-tenancy.test.ts.
const REPO_ROOT = new URL('../../../../../', import.meta.url);
const ACTION_RS = 'rs/crates/libs/paigasus-iam-core/src/authz/action.rs';

/** The wire names of `Action::as_wire`: each arm reads `Action::Name => "Name",`. */
function wireNames(source: string): Set<string> {
  const names = new Set<string>();
  for (const match of source.matchAll(/Action::\w+ => "([A-Za-z]+)",/g)) {
    if (match[1] !== undefined) names.add(match[1]);
  }
  return names;
}

const LIFECYCLE = ['RenameOrganization', 'ArchiveOrganization', 'RestoreOrganization', 'RenameTeam', 'ArchiveTeam', 'RestoreTeam', 'RenameProject', 'ArchiveProject', 'RestoreProject'];

describe('IAM_ACTIONS against the Rust action catalog', () => {
  const wire = wireNames(readFileSync(fileURLToPath(new URL(ACTION_RS, REPO_ROOT)), 'utf8'));

  // Without this floor, a changed arm format gives an empty set, and the cases below fail for the
  // wrong reason.
  it('reads a non-trivial catalog', () => {
    expect(wire.size).toBeGreaterThanOrEqual(40);
    expect(wire.has('CreateTeam')).toBe(true);
  });

  it.each([...IAM_ACTIONS])('%s is a wire name that IAM accepts', (action) => {
    expect(wire.has(action)).toBe(true);
  });

  it('holds the nine lifecycle names, and no name twice', () => {
    expect(IAM_ACTIONS).toEqual(expect.arrayContaining(LIFECYCLE));
    expect(new Set(IAM_ACTIONS).size).toBe(IAM_ACTIONS.length);
  });

  it('wireNames refuses a misspelt name', () => {
    expect(wire.has('RenameTeams')).toBe(false);
    expect(wire.has('renameTeam')).toBe(false);
  });
});
