// SPDX-License-Identifier: Apache-2.0
//
// The create-team command (spec § 5.3). It takes NO mayI: IAM decides (spec § 6.3).
import 'server-only';
import { z } from 'zod';
import { callIam, type IamClients } from '@paigasus/console-core';
import { nameField, prnField, slugField, toActionResult, type ActionResult } from '../../../../lib/form';

/** `prnField` trims: a hidden field can carry whitespace. */
export const createTeamForm = z.object({ orgPrn: prnField, slug: slugField, name: nameField });
export type CreateTeamInput = z.infer<typeof createTeamForm>;

export async function createTeam(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'createTeam'> }, input: CreateTeamInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.createTeam({ orgPrn: input.orgPrn, slug: input.slug, name: input.name })));
}
