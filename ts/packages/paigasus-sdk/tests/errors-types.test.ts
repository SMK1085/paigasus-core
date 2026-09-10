// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';

import { ErrorDomain, ErrorReason } from '../src/errors/types.js';

describe('the client-safe type surface', () => {
  // Spec § 9.6. An app may not import @paigasus/proto, so if these are not VALUES here, no
  // consumer can name a reason at all and the package fails its stated purpose.
  it('re-exports ErrorReason as a runtime value', () => {
    expect(ErrorReason.INVALID_REQUEST_SCHEMA).toBe(906);
    expect(ErrorReason.CAPABILITY_DISABLED).toBe(904);
    expect(ErrorReason.UPSTREAM_ERROR).toBe(307);
  });

  it('re-exports ErrorDomain as a runtime value', () => {
    expect(ErrorDomain.IAM).toBe(1);
    expect(ErrorDomain.GATEWAY).toBe(2);
  });
});
