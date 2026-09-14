// SPDX-License-Identifier: Apache-2.0
//
// Stands in for `next/cache` in the vitest tier (vitest.config.ts aliases it here). The real
// revalidatePath() throws outside a Next request scope. This one records each call, so a command
// test can assert what an action revalidates. Same pattern as
// ts/apps/iam-console/tests/support/next-cache.ts.

/** One recorded revalidatePath() call. `type` is `undefined` when the caller passed no scope. */
export type RevalidatedPath = { readonly path: string; readonly type: 'layout' | 'page' | undefined };

export const revalidatedPaths: RevalidatedPath[] = [];

export function revalidatePath(path: string, type?: 'layout' | 'page'): void {
  revalidatedPaths.push({ path, type });
}

export function resetNextCache(): void {
  revalidatedPaths.length = 0;
}
