// SPDX-License-Identifier: Apache-2.0
//
// MAX_BULK_REPLAY_ROWS against IAM's clamp (SMA-661 spec § 7.2, D3). IAM clamps `max_rows` to
// BulkReplayRequest::MAX_BULK_REPLAY without a signal (iam.proto:704-710), so a console ceiling
// above it would let the confirmation name a number that IAM never replays. This test reads the
// Rust constant as TEXT. moon.yml lists dead_letter.rs as an input of this package's `test` task,
// so a Rust edit selects this test (the iam-console `test` task has no /rs/** input, § 7.2).
//
// NOT VACUOUS. The type's doc comment in the same file names `MAX_BULK_REPLAY` and `(10_000)` in
// prose (dead_letter.rs:60-62), so a loose pattern reads 10_000 from the comment with the constant
// DELETED. The pattern is line-anchored on the `pub const` declaration, exactly one match is
// required, and `_` is stripped before Number(): Number('10_000') is NaN.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { MAX_BULK_REPLAY_ROWS } from '../../src/form';

// tests/unit -> tests -> paigasus-console-core -> packages -> ts -> repo root: five `../`, the same
// depth as tests/unit/action-names.test.ts.
const REPO_ROOT = new URL('../../../../../', import.meta.url);
const DEAD_LETTER_RS = 'rs/crates/libs/paigasus-iam-core/src/dead_letter.rs';
const DECLARATION = /^\s*pub const MAX_BULK_REPLAY: u64 = ([0-9_]+);/gm;

describe('MAX_BULK_REPLAY_ROWS against the Rust clamp', () => {
  const matches = [...readFileSync(fileURLToPath(new URL(DEAD_LETTER_RS, REPO_ROOT)), 'utf8').matchAll(DECLARATION)];

  it('finds exactly one declaration of the constant', () => {
    expect(matches).toHaveLength(1);
  });

  it('equals the Rust value', () => {
    const value = Number((matches[0]?.[1] ?? '').replaceAll('_', ''));
    expect(Number.isInteger(value)).toBe(true);
    expect(value).toBeGreaterThan(0);
    expect(MAX_BULK_REPLAY_ROWS).toBe(value);
  });
});
