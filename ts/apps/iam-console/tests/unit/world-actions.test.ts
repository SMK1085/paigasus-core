// SPDX-License-Identifier: Apache-2.0
//
// The e2e world's ALL_ACTIONS against @paigasus/console-core's IAM_ACTIONS (SMA-630 spec § 5.1). The
// world allows ALL_ACTIONS by default. A name missing here would make the e2e tier deny it in
// silence, and a control that the default world should show would stay hidden.
import { describe, expect, it } from 'vitest';
import { IAM_ACTIONS } from '@paigasus/console-core';
import { ALL_ACTIONS } from '../e2e/support/world';

describe('ALL_ACTIONS', () => {
  it('holds the same set as IAM_ACTIONS', () => {
    expect([...ALL_ACTIONS].sort()).toEqual([...IAM_ACTIONS].sort());
    expect(new Set(ALL_ACTIONS).size).toBe(ALL_ACTIONS.length);
  });
});
