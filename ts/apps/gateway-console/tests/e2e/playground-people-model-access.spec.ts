// SPDX-License-Identifier: Apache-2.0
//
// SMA-676 spec § 7.3: ONE identity. In the `orgCreator` world the signed-in admin created the
// organization: it holds org_admin there and is not a member, so the section offers it as a
// candidate (D12). The admin grants model access to themself, the playground answers, the admin
// revokes, and the playground refuses. The file name starts with `playground` so the playground
// project (the real gateway binary) runs it. The fake IAM is not Cedar (playground-authz.spec.ts:3):
// this row proves the wiring; rs/crates/services/paigasus-iam/tests/authz_people_model_access.rs
// proves the decision.
import type { Page } from '@playwright/test';
import { signIn, waitForHydration } from './support/login';
import { expect, test } from './support/playground-harness';
import { ORG_ID, PRINCIPAL_PRN } from './support/world';

const ORG_PATH = `/gateway/orgs/${ORG_ID}`;
const PLAYGROUND = `${ORG_PATH}/playground`;
// app/_components/playground.tsx MISSING_ROLE_TEXT. Copied, not imported: that module is a React
// client component, and this file runs under plain Playwright.
const MISSING_ROLE_TEXT = 'You need the gateway_user role on this organization. Ask an organization admin to grant it.';

async function send(page: Page): Promise<void> {
  await page.getByLabel('Model').fill('gpt-e2e');
  await page.getByLabel('Message').fill('hello');
  await page.getByRole('button', { name: 'Send' }).click();
}

test('R30: the org creator grants model access to themself, the playground answers, and after a revoke it refuses (SMA-676 § 7.3)', async ({ page, harness }) => {
  harness.useWorld({ orgCreator: true });
  await signIn(page, harness, ORG_PATH);
  const section = page.getByTestId('people-model-access');
  await expect(section.getByTestId('people-holder-row')).toHaveCount(0);

  await section.getByRole('combobox').click();
  await page.getByRole('option', { name: PRINCIPAL_PRN }).click();
  await section.getByRole('button', { name: 'Grant model access' }).click();
  await expect(section.getByTestId('people-holder-row')).toHaveCount(1);
  await expect(section.getByTestId('people-holder-row')).toHaveAttribute('data-principal', PRINCIPAL_PRN);
  await expect(section.getByTestId('people-not-member')).toHaveCount(1);

  await page.goto(harness.url(PLAYGROUND));
  await waitForHydration(page);
  await send(page);
  await expect(page.getByTestId('assistant-turn').last()).toHaveAttribute('data-status', 'done');

  await page.goto(harness.url(ORG_PATH));
  await waitForHydration(page);
  await section.getByRole('button', { name: 'Revoke' }).click();
  await expect(section.getByTestId('people-holder-row')).toHaveCount(0);

  await page.goto(harness.url(PLAYGROUND));
  await waitForHydration(page);
  await send(page);
  await expect(page.getByTestId('playground-error')).toContainText(MISSING_ROLE_TEXT);
});
