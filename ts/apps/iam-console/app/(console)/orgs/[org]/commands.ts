// SPDX-License-Identifier: Apache-2.0
//
// The commands of the organization page (spec § 5.3): create a team, and rename, archive and
// restore the organization (SMA-630 spec § 4). They take NO mayI: IAM decides (spec § 6.3).
import 'server-only';
import { z } from 'zod';
import { callIam, type IamClients } from '@paigasus/console-core';
import { nameField, prnField, renameChange, renameForm, slugField, toActionResult, type ActionResult } from '../../../../lib/form';

/** `prnField` trims: a hidden field can carry whitespace. */
export const createTeamForm = z.object({ orgPrn: prnField, slug: slugField, name: nameField });
export type CreateTeamInput = z.infer<typeof createTeamForm>;

export async function createTeam(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'createTeam'> }, input: CreateTeamInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.createTeam({ orgPrn: input.orgPrn, slug: input.slug, name: input.name })));
}

/**
 * The rename form holds the current values as hidden fields, so the command sends only what the
 * user changed (SMA-630 spec D6). A changed hidden `prn` is safe: IAM authorizes the action against
 * the STORED node that the PRN names (spec § 4.2).
 */
export const renameOrganizationForm = renameForm();
export type RenameOrganizationInput = z.infer<typeof renameOrganizationForm>;

export async function renameOrganization(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'renameOrganization'> }, input: RenameOrganizationInput): Promise<ActionResult> {
  const change = renameChange(input);
  return toActionResult(await callIam(() => deps.tenancy.renameOrganization({ prn: input.prn, ...change })));
}

export const archiveOrganizationForm = z.object({ prn: prnField });
export type ArchiveOrganizationInput = z.infer<typeof archiveOrganizationForm>;

export async function archiveOrganization(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'archiveOrganization'> }, input: ArchiveOrganizationInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.archiveOrganization({ prn: input.prn })));
}

export const restoreOrganizationForm = archiveOrganizationForm;
export type RestoreOrganizationInput = z.infer<typeof restoreOrganizationForm>;

export async function restoreOrganization(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'restoreOrganization'> }, input: RestoreOrganizationInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.restoreOrganization({ prn: input.prn })));
}
