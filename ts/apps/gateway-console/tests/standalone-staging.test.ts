// SPDX-License-Identifier: Apache-2.0
//
// The build-side pin of SMA-655. `gateway-console-ts:build` stages .next/static into the standalone
// tree (moon.yml), and this reds if a later edit removes that staging. It sits next to
// standalone-runtime.test.ts because both read the real build output; `test` depends on
// `~:build`, and lists moon.yml as an input so a moon.yml-only edit selects it.
import { describe, it } from 'vitest';
import { APP_DIR, STANDALONE_APP_DIR } from './e2e/support/paths';
import { assertStagedBuild } from './e2e/support/staged-build';

describe('the build stages the standalone tree (SMA-655)', () => {
  it("holds this build's static tree", () => {
    assertStagedBuild(APP_DIR, STANDALONE_APP_DIR, 'gateway-console-ts:build');
  });
});
