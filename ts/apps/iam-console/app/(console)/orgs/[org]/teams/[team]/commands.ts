// SPDX-License-Identifier: Apache-2.0
//
// The commands of the team page (spec § 5.3): create a project, and rename, archive and restore the
// team (SMA-630 spec § 4). They take NO mayI: IAM decides (spec § 6.3).
import 'server-only';
import { z } from 'zod';
import { callIam, type IamClients } from '@paigasus/console-core';
import { nameField, prnField, renameChange, renameForm, slugField, toActionResult, type ActionResult } from '../../../../../../lib/form';

/** `prnField` trims: a hidden field can carry whitespace. */
export const createProjectForm = z.object({ teamPrn: prnField, slug: slugField, name: nameField });
export type CreateProjectInput = z.infer<typeof createProjectForm>;

export async function createProject(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'createProject'> }, input: CreateProjectInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.createProject({ teamPrn: input.teamPrn, slug: input.slug, name: input.name })));
}

/** See ../../commands.ts: the rename form holds the current values, and the command sends only the changes (D6). */
export const renameTeamForm = renameForm();
export type RenameTeamInput = z.infer<typeof renameTeamForm>;

export async function renameTeam(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'renameTeam'> }, input: RenameTeamInput): Promise<ActionResult> {
  const change = renameChange(input);
  return toActionResult(await callIam(() => deps.tenancy.renameTeam({ prn: input.prn, ...change })));
}

export const archiveTeamForm = z.object({ prn: prnField });
export type ArchiveTeamInput = z.infer<typeof archiveTeamForm>;

export async function archiveTeam(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'archiveTeam'> }, input: ArchiveTeamInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.archiveTeam({ prn: input.prn })));
}

export const restoreTeamForm = archiveTeamForm;
export type RestoreTeamInput = z.infer<typeof restoreTeamForm>;

export async function restoreTeam(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'restoreTeam'> }, input: RestoreTeamInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.restoreTeam({ prn: input.prn })));
}
