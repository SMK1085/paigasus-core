// SPDX-License-Identifier: Apache-2.0
//
// The create-team command (spec § 5.3). It takes NO mayI: IAM decides (spec § 6.3).
import 'server-only';
import { z } from 'zod';
import { callIam } from '../../../../lib/errors';
import { toActionResult, type ActionResult } from '../../../../lib/form';
import type { IamClients } from '../../../../lib/iam';

const text = z.string().trim().min(1).max(200);

export const createTeamForm = z.object({ orgPrn: z.string().min(1).max(512), slug: text, name: text });
export type CreateTeamInput = z.infer<typeof createTeamForm>;

export async function createTeam(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'createTeam'> }, input: CreateTeamInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.createTeam({ orgPrn: input.orgPrn, slug: input.slug, name: input.name })));
}
