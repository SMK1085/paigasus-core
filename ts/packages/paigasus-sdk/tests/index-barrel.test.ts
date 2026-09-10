// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';

import * as barrel from '../src/index.js';

describe('the root barrel serves every entry surface', () => {
  it.each(['createChatClient', 'createTerminalFrameParser', 'mapError', 'createIamClient', 'disposeTransports', 'ErrorReason', 'ErrorDomain', 'presentationForGrpcCode', 'presentationForHttpStatus'])(
    'exports %s',
    (name) => {
      expect(barrel).toHaveProperty(name);
      expect(barrel[name as keyof typeof barrel]).toBeDefined();
    },
  );

  // An ambiguous star-exported name is dropped SILENTLY under ES semantics, so a value that is
  // present here can still vanish when a second module grows the same name. Naming each
  // re-export explicitly is what prevents it; this test is the reminder, not the mechanism.
  it('exposes ErrorReason as the registry enum, not a shadow', () => {
    expect(barrel.ErrorReason.UPSTREAM_ERROR).toBe(307);
  });
});
