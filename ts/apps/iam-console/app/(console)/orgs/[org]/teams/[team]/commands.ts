// SPDX-License-Identifier: Apache-2.0
//
// The create-project command (spec § 5.3). It takes NO mayI: IAM decides (spec § 6.3).
import 'server-only';
import { z } from 'zod';
import { callIam } from '../../../../../../lib/errors';
import { toActionResult, type ActionResult } from '../../../../../../lib/form';
import type { IamClients } from '../../../../../../lib/iam';

const text = z.string().trim().min(1).max(200);

/** `.trim()` like every other PRN field (../../../commands.ts's `prn`): a hidden field can carry whitespace. */
export const createProjectForm = z.object({ teamPrn: z.string().trim().min(1).max(512), slug: text, name: text });
export type CreateProjectInput = z.infer<typeof createProjectForm>;

export async function createProject(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'createProject'> }, input: CreateProjectInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.createProject({ teamPrn: input.teamPrn, slug: input.slug, name: input.name })));
}
