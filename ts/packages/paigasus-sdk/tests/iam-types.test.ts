// SPDX-License-Identifier: Apache-2.0
//
// SMA-630 spec § 4.5 (D8), SMA-636 spec § 5.7. An app may not import @paigasus/proto, so the
// loaders and the e2e worlds can name a node status or an API key status only through this
// guard-free entry. Each must be the registry enum ITSELF, as a runtime value, or a screen compares
// against a copy that can drift.
import { ApiKeyStatus as ProtoApiKeyStatus, NodeStatus as ProtoNodeStatus, PrincipalKind as ProtoPrincipalKind } from '@paigasus/proto/iam';
import { describe, expect, it } from 'vitest';

import { ApiKeyStatus, NodeStatus, PrincipalKind } from '../src/iam/types.js';

describe('the guard-free ./iam/types entry', () => {
  it('re-exports NodeStatus as a runtime value', () => {
    expect(NodeStatus.UNSPECIFIED).toBe(0);
    expect(NodeStatus.ACTIVE).toBe(1);
    expect(NodeStatus.ARCHIVED).toBe(2);
  });

  it('re-exports the registry enum object, not a copy', () => {
    expect(NodeStatus).toBe(ProtoNodeStatus);
  });

  it('re-exports ApiKeyStatus as the registry enum object (SMA-636)', () => {
    expect(ApiKeyStatus.UNSPECIFIED).toBe(0);
    expect(ApiKeyStatus.ACTIVE).toBe(1);
    expect(ApiKeyStatus.REVOKED).toBe(2);
    expect(ApiKeyStatus).toBe(ProtoApiKeyStatus);
  });

  it('re-exports PrincipalKind as the registry enum object (SMA-676)', () => {
    expect(PrincipalKind.UNSPECIFIED).toBe(0);
    expect(PrincipalKind.USER).toBe(1);
    expect(PrincipalKind.SERVICE_ACCOUNT).toBe(2);
    expect(PrincipalKind).toBe(ProtoPrincipalKind);
  });
});
