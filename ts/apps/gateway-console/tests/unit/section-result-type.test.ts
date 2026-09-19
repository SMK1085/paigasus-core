// SPDX-License-Identifier: Apache-2.0
//
// SectionResult cannot hold a token (SMA-636 spec § 6.1). The token lives in ONE browser place,
// TokenPanel's own useState. The result region's `issue` arm holds a failure only, so an issue
// SUCCESS is not assignable to it, and a future setResult cannot keep a second copy of the token.
// The proof is a type-level one: `pnpm typecheck` fails when a `@ts-expect-error` below has no error.
import { describe, expect, expectTypeOf, it } from 'vitest';
import type { IssueKeyState, SectionResult } from '../../app/(console)/service-accounts/view';

type IssueSuccess = Extract<Exclude<IssueKeyState, null>, { ok: true }>;
type IssueFailure = Extract<Exclude<IssueKeyState, null>, { ok: false }>;

describe('SectionResult (§ 6.1)', () => {
  it('does not accept an issue success, which carries the token', () => {
    const success: IssueSuccess = { ok: true, token: 'pgs_type_only', prefix: 'pgs_type' };
    // @ts-expect-error -- an issue success carries the token and must never be a SectionResult.
    const stored: SectionResult = { control: 'issue', state: success };
    expectTypeOf<{ control: 'issue'; state: IssueSuccess }>().not.toExtend<SectionResult>();
    expect(stored).not.toBeNull();
  });

  it('accepts an issue failure', () => {
    expectTypeOf<{ readonly control: 'issue'; readonly state: IssueFailure }>().toExtend<SectionResult>();
  });
});
