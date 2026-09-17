// SPDX-License-Identifier: Apache-2.0
//
// The create-project command (spec § 5.3). It takes NO mayI: IAM decides (spec § 6.3).
import 'server-only';
import { z } from 'zod';
import { callIam, type IamClients } from '@paigasus/console-core';
import { nameField, prnField, slugField, toActionResult, type ActionResult } from '../../../../../../lib/form';

/** `prnField` trims: a hidden field can carry whitespace. */
export const createProjectForm = z.object({ teamPrn: prnField, slug: slugField, name: nameField });
export type CreateProjectInput = z.infer<typeof createProjectForm>;

export async function createProject(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'createProject'> }, input: CreateProjectInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.createProject({ teamPrn: input.teamPrn, slug: input.slug, name: input.name })));
}
