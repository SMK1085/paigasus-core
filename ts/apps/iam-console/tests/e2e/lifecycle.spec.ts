// SPDX-License-Identifier: Apache-2.0
//
// Rename, archive and restore (SMA-630 spec § 9.3, rows R14–R16).
//
// R14 scripts STATEFUL handlers: the state lives in the test's closure, and each call changes both
// `status` and `effectiveStatus`, as a real archive of the node itself does. Every button locator
// uses `exact: true`, because "Confirm archive" contains "archive".
import { ErrorReason } from '@paigasus/sdk/errors/types';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import { denial } from '@paigasus/console-core/testing';
import { FORM_REASON_COPY } from '../../app/_components/error-copy';
import { BADGE_LABEL, PARENT_ARCHIVED_NOTE } from '../../app/(console)/node-status';
import { signIn, waitForHydration } from './support/login';
import { ALL_ACTIONS, ORG_ID, ORG_PRN, PROJECT_ID, PROJECT_NAME, PROJECT_PRN, TEAM_ID, TEAM_NAME, TEAM_PRN } from './support/world';
import { expect, test } from './support/harness';

const TEAM_PATH = `/iam/orgs/${ORG_ID}/teams/${TEAM_ID}`;
const PROJECT_PATH = `${TEAM_PATH}/projects/${PROJECT_ID}`;

test('R14: archive and restore a team, with a two-step archive (SMA-630)', async ({ page, harness }) => {
  await signIn(page, harness);
  let status: NodeStatus = NodeStatus.ACTIVE;
  const team = () => ({ prn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'platform', name: TEAM_NAME, status, effectiveStatus: status });
  harness.useWorld({
    overrides: {
      'tenancy.getTeam': () => ({ team: team() }),
      'tenancy.archiveTeam': () => {
        status = NodeStatus.ARCHIVED;
        return { team: team() };
      },
      'tenancy.restoreTeam': () => {
        status = NodeStatus.ACTIVE;
        return { team: team() };
      },
    },
  });
  await page.goto(harness.url(TEAM_PATH));
  await waitForHydration(page);
  const archivesBefore = harness.iam.callsTo('tenancy.archiveTeam').length;
  const restoresBefore = harness.iam.callsTo('tenancy.restoreTeam').length;
  const badge = page.getByTestId('node-status');
  const archive = page.getByRole('button', { name: 'Archive', exact: true });
  const confirm = page.getByRole('button', { name: 'Confirm archive', exact: true });
  const restore = page.getByRole('button', { name: 'Restore', exact: true });
  await expect(badge).toHaveCount(0);

  // Step 1: the two-step archive.
  await archive.click();
  await confirm.click();
  await expect(badge).toHaveText(BADGE_LABEL.archived ?? '');
  await expect(restore).toBeVisible();
  await expect(archive).toHaveCount(0);
  await expect(confirm).toHaveCount(0);

  // Step 2: restore. The confirmation state of the first control is not kept.
  await restore.click();
  await expect(badge).toHaveCount(0);
  await expect(archive).toBeVisible();
  await expect(confirm).toHaveCount(0);

  // Step 3: exactly one call of each, with the team PRN.
  const prnOf = (call: { request: unknown }): string => (call.request as { prn: string }).prn;
  expect(harness.iam.callsTo('tenancy.archiveTeam').slice(archivesBefore).map(prnOf)).toEqual([TEAM_PRN]);
  expect(harness.iam.callsTo('tenancy.restoreTeam').slice(restoresBefore).map(prnOf)).toEqual([TEAM_PRN]);
});

test('R15: a denied rename shows an inline 403 and keeps the typed slug (SMA-630)', async ({ page, harness }) => {
  await signIn(page, harness);
  harness.useWorld({
    overrides: {
      'tenancy.renameProject': () => {
        throw denial({ correlationId: 'corr-e2e-rename-403' });
      },
    },
  });
  await page.goto(harness.url(PROJECT_PATH));
  await waitForHydration(page);
  const before = harness.iam.callsTo('tenancy.renameProject').length;
  const copy = FORM_REASON_COPY[ErrorReason.FORBIDDEN];
  if (copy === undefined) throw new Error('FORM_REASON_COPY has no FORBIDDEN entry');

  const form = page.getByTestId('rename-project');
  await form.getByLabel('Slug').fill('renamed-gateway');
  await form.getByRole('button', { name: 'Rename', exact: true }).click();

  const error = form.getByTestId('rename-project-error');
  await expect(error).toContainText(copy);
  // A form 403 shows the id in BOTH correlation modes (FormError).
  await expect(error.getByTestId('correlation-id')).toHaveText('corr-e2e-rename-403');
  await expect(form.getByLabel('Slug')).toHaveValue('renamed-gateway');
  await expect(form.getByLabel('Name')).toHaveValue(PROJECT_NAME);
  expect(new URL(page.url()).pathname).toBe(PROJECT_PATH);
  const renamed = harness.iam.callsTo('tenancy.renameProject').slice(before);
  expect(renamed).toHaveLength(1);
  expect(renamed[0]?.request).toMatchObject({ prn: PROJECT_PRN, newSlug: 'renamed-gateway' });
  expect((renamed[0]?.request as { newName?: string } | undefined)?.newName).toBeUndefined();
});

test('R16: a project under an archived parent shows the parent badge and the note, and no control (SMA-630)', async ({ page, harness }) => {
  await signIn(page, harness);
  // The starter policy `forbid-archived-writes` denies rename and archive on an effectively
  // archived node, and allows restore (spec F7, F9).
  harness.useWorld({
    allow: ALL_ACTIONS.filter((action) => action !== 'RenameProject' && action !== 'ArchiveProject'),
    overrides: {
      'tenancy.getProject': () => ({
        project: { prn: PROJECT_PRN, teamPrn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'gateway', name: PROJECT_NAME, status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ARCHIVED },
      }),
    },
  });

  await page.goto(harness.url(PROJECT_PATH));
  await waitForHydration(page);

  await expect(page.getByTestId('node-status')).toHaveText(BADGE_LABEL['archived-parent'] ?? '');
  await expect(page.getByTestId('manage-note')).toHaveText(PARENT_ARCHIVED_NOTE);
  await expect(page.getByTestId('rename-project')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Archive', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Restore', exact: true })).toHaveCount(0);
});
