// SPDX-License-Identifier: Apache-2.0

/*
 * The AC-1 negative control. This module deliberately violates the next/* ban so that
 * tests/no-next-imports.test.ts can prove the resolver plugin in vitest.config.ts actually
 * fires. It is never imported by src/.
 *
 * The @ts-expect-error is load-bearing twice over. It suppresses the "cannot find module"
 * error tsc raises today — and if `next` ever BECOMES resolvable from this package, the
 * directive turns unused and tsc reds on that instead. So this line also guards pnpm's
 * isolation, which is the incidental protection described in vitest.config.ts.
 *
 * The eslint-disable is the third control on the same line, and the narrowest possible
 * exception to SMA-502's boundary rule. That rule bans next/* across packages/paigasus-ui/**,
 * and it is RIGHT to cover tests/ as well as src/ — a test importing next/link would pull a
 * Next runtime into the jsdom tier, which is exactly what AC 2 forbids. This one file is the
 * exception because its whole job is to violate the ban so something can observe the violation
 * being caught. Nothing in src/ imports it, and no other fixture may carry this disable: if a
 * second one appears, the rule is being worked around rather than excepted.
 */
// The eslint directive is a trailing `disable-line`, not a `disable-next-line`, because
// `@ts-expect-error` must be the line immediately above the import and only one of them can
// hold that slot.
// @ts-expect-error `next` is deliberately absent from @paigasus/ui's dependencies.
import NextLink from 'next/link'; // eslint-disable-line no-restricted-imports -- the fixture must violate the ban for the test to observe it being caught

export default NextLink;
