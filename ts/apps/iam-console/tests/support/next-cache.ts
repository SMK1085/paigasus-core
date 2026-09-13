// SPDX-License-Identifier: Apache-2.0
//
// Stands in for `next/cache` in the vitest tier (vitest.config.ts aliases it here). The real
// revalidatePath() throws outside a Next request scope. This one records each call, so a command
// test can assert what an action revalidates.
//
// IT RECORDS BOTH ARGUMENTS (review, defect 4). The double used to drop the second one, so the
// `'layout'` scope — the thing that refreshes the list under /orgs after a create, where `'page'`
// refreshes only the one route — could regress with no test anywhere able to see it.

/** One recorded revalidatePath() call. `type` is `undefined` when the caller passed no scope. */
export type RevalidatedPath = { readonly path: string; readonly type: 'layout' | 'page' | undefined };

export const revalidatedPaths: RevalidatedPath[] = [];

export function revalidatePath(path: string, type?: 'layout' | 'page'): void {
  revalidatedPaths.push({ path, type });
}

export function resetNextCache(): void {
  revalidatedPaths.length = 0;
}
