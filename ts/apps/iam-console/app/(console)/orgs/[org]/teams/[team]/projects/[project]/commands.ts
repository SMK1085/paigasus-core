// SPDX-License-Identifier: Apache-2.0
//
// The commands of the project page: rename, archive and restore the project (SMA-630 spec § 4). They
// take NO mayI: IAM decides (spec § 6.3). The rename form holds the current values, and the command
// sends only the changes (D6).
import 'server-only';
import { z } from 'zod';
import { callIam, type IamClients } from '@paigasus/console-core';
import { prnField, renameChange, renameForm, toActionResult, type ActionResult } from '../../../../../../../../lib/form';

export const renameProjectForm = renameForm();
export type RenameProjectInput = z.infer<typeof renameProjectForm>;

export async function renameProject(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'renameProject'> }, input: RenameProjectInput): Promise<ActionResult> {
  const change = renameChange(input);
  return toActionResult(await callIam(() => deps.tenancy.renameProject({ prn: input.prn, ...change })));
}

export const archiveProjectForm = z.object({ prn: prnField });
export type ArchiveProjectInput = z.infer<typeof archiveProjectForm>;

export async function archiveProject(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'archiveProject'> }, input: ArchiveProjectInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.archiveProject({ prn: input.prn })));
}

export const restoreProjectForm = archiveProjectForm;
export type RestoreProjectInput = z.infer<typeof restoreProjectForm>;

export async function restoreProject(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'restoreProject'> }, input: RestoreProjectInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.restoreProject({ prn: input.prn })));
}
