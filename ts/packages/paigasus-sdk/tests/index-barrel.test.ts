// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';

import * as barrel from '../src/index.js';
import * as chat from '../src/chat.js';
import * as errors from '../src/errors.js';
import * as iam from '../src/iam.js';
import * as iamTypes from '../src/iam/types.js';
import type { ChatCallOptions } from '../src/index.js';

/**
 * Every runtime key of each submodule, derived rather than hand-listed.
 *
 * The old version of this test sampled nine names by hand out of twenty-two real exports — an
 * omission from the barrel still compiled and shipped silently. A module namespace object's own
 * keys ARE its runtime export names (type-only exports are erased entirely under
 * `verbatimModuleSyntax`), so deriving from `Object.keys` cannot go stale the way a hand list can.
 */
const SUBMODULES = { chat, errors, iam, 'iam/types': iamTypes } as const;
const ENTRIES = Object.entries(SUBMODULES).flatMap(([moduleName, ns]) => Object.keys(ns).map((key) => [moduleName, key] as const));

describe('the root barrel serves every entry surface', () => {
  it('has at least as many cases as the old hand-written list', () => {
    // A floor, not a ceiling — guards against a refactor accidentally emptying SUBMODULES above,
    // which would make every row below vacuously pass zero times.
    expect(ENTRIES.length).toBeGreaterThanOrEqual(9);
  });

  // IDENTITY, not merely presence. A barrel entry pointing at the wrong module's same-named value
  // would satisfy `toBeDefined()` while serving something else entirely — which is exactly the
  // silent shadowing the explicit re-export list exists to prevent.
  it.each(ENTRIES)('the barrel re-exports ./%s key %s', (moduleName, key) => {
    expect(barrel).toHaveProperty(key);
    expect(barrel[key as keyof typeof barrel]).toBe(SUBMODULES[moduleName as keyof typeof SUBMODULES][key as never]);
  });

  // An ambiguous star-exported name is dropped SILENTLY under ES semantics, so a value that is
  // present here can still vanish when a second module grows the same name. Naming each
  // re-export explicitly is what prevents it; this test is the reminder, not the mechanism.
  it('exposes ErrorReason as the registry enum, not a shadow', () => {
    expect(barrel.ErrorReason.UPSTREAM_ERROR).toBe(307);
  });

  it('exposes NodeStatus as the registry enum (SMA-630)', () => {
    expect(barrel.NodeStatus.ARCHIVED).toBe(2);
  });

  // ChatCallOptions is a TYPE-only export, erased at runtime, so it cannot appear in ENTRIES
  // above (which walks Object.keys of the runtime namespace). This is a compile-time check
  // instead: `tsc` fails if the barrel drops the type export, since the literal below would then
  // no longer satisfy an imported type.
  it('exposes ChatCallOptions as a type from the root barrel', () => {
    const options: ChatCallOptions = { org: 'org-1' };
    expect(options.org).toBe('org-1');
  });
});
