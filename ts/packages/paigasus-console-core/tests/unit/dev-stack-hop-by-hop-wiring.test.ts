// SPDX-License-Identifier: Apache-2.0
//
// SMA-640 spec § 6.4. ts/tooling/dev-stack.ts cannot run in a test (Docker plus two `next dev`
// servers; its own header says so), so this file pins its default-zone proxy's call sites as TEXT:
// both directions go through the shared forwardableHeaders(), and no local copy of the rule comes
// back. moon.yml lists /ts/tooling/dev-stack.ts as an input of this package's `test` task, so an
// edit to dev-stack.ts re-runs this file rather than serving a cached pass.
//
// Comment lines are removed before any match, so a comment that names a call cannot satisfy the
// pin (plan decision P3).
//
// RESIDUAL (spec § 10): a text pin cannot prove runtime behaviour. A change that keeps these
// strings but adds a second, unfiltered httpRequest reds nothing here.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

// tests/unit -> tests -> paigasus-console-core -> packages -> ts -> repo root: five `../`, the same
// depth as tests/unit/bulk-replay-ceiling.test.ts.
const REPO_ROOT = new URL('../../../../../', import.meta.url);
const DEV_STACK = 'ts/tooling/dev-stack.ts';

const source = readFileSync(fileURLToPath(new URL(DEV_STACK, REPO_ROOT)), 'utf8');
const code = source
  .split('\n')
  .filter((line) => !/^\s*(\/\/|\/\*|\*)/.test(line))
  .join('\n');

function count(needle: string): number {
  return code.split(needle).length - 1;
}

describe('dev-stack.ts uses the shared hop-by-hop rule', () => {
  it('reads a non-empty file', () => {
    expect(code.length).toBeGreaterThan(1000);
  });

  it('imports forwardableHeaders from @paigasus/console-core/testing', () => {
    expect(code).toMatch(/^import \{[^}]*\bforwardableHeaders\b[^}]*\} from '@paigasus\/console-core\/testing';$/m);
  });

  it('filters the request headers exactly once', () => {
    expect(count('forwardableHeaders(req.headers)')).toBe(1);
  });

  it('filters the response headers exactly once', () => {
    expect(count('forwardableHeaders(upstreamRes.headers)')).toBe(1);
  });

  it('keeps no local copy of the rule', () => {
    expect(code).not.toContain('HOP_BY_HOP');
    expect(code).not.toContain('function forwardable(');
  });
});
