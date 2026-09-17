// SPDX-License-Identifier: Apache-2.0
//
// SMA-630 spec § 4.5 (D8). An app may not import @paigasus/proto, so the loaders and the e2e world
// can name a node status only through this guard-free entry. It must be the registry enum ITSELF,
// as a runtime value, or a screen compares against a copy that can drift.
import { NodeStatus as ProtoNodeStatus } from '@paigasus/proto/iam';
import { describe, expect, it } from 'vitest';

import { NodeStatus } from '../src/iam/types.js';

describe('the guard-free ./iam/types entry', () => {
  it('re-exports NodeStatus as a runtime value', () => {
    expect(NodeStatus.UNSPECIFIED).toBe(0);
    expect(NodeStatus.ACTIVE).toBe(1);
    expect(NodeStatus.ARCHIVED).toBe(2);
  });

  it('re-exports the registry enum object, not a copy', () => {
    expect(NodeStatus).toBe(ProtoNodeStatus);
  });
});
