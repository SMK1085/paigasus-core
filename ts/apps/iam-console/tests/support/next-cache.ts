// SPDX-License-Identifier: Apache-2.0
//
// Stands in for `next/cache` in the vitest tier (vitest.config.ts aliases it here). The real
// revalidatePath() throws outside a Next request scope. This one records each call, so a command
// test can assert what an action revalidates.
export const revalidatedPaths: string[] = [];

export function revalidatePath(path: string): void {
  revalidatedPaths.push(path);
}

export function resetNextCache(): void {
  revalidatedPaths.length = 0;
}
